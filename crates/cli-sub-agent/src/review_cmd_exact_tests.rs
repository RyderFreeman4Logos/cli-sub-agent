use std::path::Path;
use std::process::Command;

use csa_config::{GlobalConfig, ProjectProfile, TierStrategy, config::TierConfig};
use csa_core::types::{ReviewDecision, ToolName};
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity,
    write_findings_toml,
};
use std::collections::HashMap;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn exact_test_run_git(repo: &Path, args: &[&str]) {
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


fn exact_test_setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    exact_test_run_git(temp.path(), &["init"]);
    exact_test_run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    exact_test_run_git(temp.path(), &["config", "user.name", "Test User"]);

    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    exact_test_run_git(temp.path(), &["add", "tracked.txt"]);
    exact_test_run_git(temp.path(), &["commit", "-m", "initial"]);

    temp
}

fn exact_test_project_config_with_enabled_tools(tools: &[&str]) -> csa_config::ProjectConfig {
    let mut tool_map = std::collections::HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            csa_config::ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            csa_config::ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    csa_config::ProjectConfig {
        schema_version: 1,
        project: csa_config::ProjectMeta::default(),
        resources: csa_config::ResourcesConfig {
            memory_max_mb: Some(1024),
            min_free_memory_mb: 1,
            soft_limit_percent: tools.contains(&"codex").then_some(100),
            ..Default::default()
        },
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: std::collections::HashMap::new(),
        tier_mapping: std::collections::HashMap::new(),
        aliases: std::collections::HashMap::new(),
        tool_aliases: std::collections::HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

include!("review_cmd_exact_test_support.rs");

fn exact_test_config_with_review_tier(
    enabled_tools: &[&str],
    models: &[&str],
) -> csa_config::ProjectConfig {
    let mut config = exact_test_project_config_with_enabled_tools(enabled_tools);
    if enabled_tools.contains(&"codex") {
        exact_test_configure_codex_cli_review_test_tool(&mut config);
    }
    config.tiers.insert(
        "quality".to_string(),
        TierConfig {
            description: "quality".to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config
}

fn exact_test_write_executable(bin_dir: &Path, name: &str, body: &str) {
    let path = bin_dir.join(name);
    std::fs::write(&path, body).expect("write fake binary");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn exact_test_make_review_meta(
    session_id: &str,
    decision: ReviewDecision,
    verdict: &str,
) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: decision.as_str().to_string(),
        verdict: verdict.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: if decision == ReviewDecision::Pass { 0 } else { 1 },
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    }
}

fn exact_test_make_review_finding(severity: Severity, id: &str) -> ReviewFinding {
    ReviewFinding {
        id: id.to_string(),
        severity,
        file_ranges: vec![ReviewFindingFileRange {
            path: "src/lib.rs".to_string(),
            start: 1,
            end: Some(1),
        }],
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: format!("description {id}"),
    }
}

fn exact_test_create_review_session(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    description: &str,
) -> (String, std::path::PathBuf) {
    let mut session =
        csa_session::create_session_fresh(project_root, Some(description), None, Some("codex"))
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

fn exact_test_codex_agent_message(text: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "item.completed",
        "item": {
            "type": "agent_message",
            "text": text,
        }
    }))
    .expect("serialize transcript line")
}

fn exact_test_wait_result(exit_code: i32, summary: &str) -> csa_session::SessionResult {
    let now = chrono::Utc::now();
    csa_session::SessionResult {
        post_exec_gate: None,
        status: csa_session::SessionResult::status_from_exit_code(exit_code),
        exit_code,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(1),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    }
}

#[test]
fn fix_loop_exhausted_preserves_open_findings_in_findings_toml() {
    let project_dir = exact_test_setup_git_repo();
    let _state_home = test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let session_id = "01TESTFIXLOOPEXACT000000000";
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    let expected = FindingsFile {
        findings: vec![exact_test_make_review_finding(Severity::High, "open-high")],
    };
    write_findings_toml(&session_dir, &expected).expect("write last-round findings.toml");

    let mut exhausted_meta =
        exact_test_make_review_meta(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    exhausted_meta.fix_rounds = 3;

    review_cmd::persist_fix_final_artifacts_for_tests(project_dir.path(), &exhausted_meta, false);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = std::fs::read_to_string(&findings_path).expect("read preserved findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse preserved findings.toml");
    assert_eq!(parsed, expected);
}

#[test]
fn persist_verdict_refreshes_on_fix_reuse_session() {
    let project_dir = exact_test_setup_git_repo();
    let _state_home = test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let session_id = "01TESTVERDICTEXACT000000000";
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let stale_artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    std::fs::write(
        &verdict_path,
        serde_json::to_vec_pretty(&stale_artifact).expect("serialize stale verdict"),
    )
    .expect("write stale verdict");
    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: Vec::new(),
        },
    )
    .expect("write refreshed findings.toml");
    let full_output = [
        serde_json::json!({"type":"item.completed","item":{
            "id":"item_1",
            "type":"agent_message",
            "text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking issues found in this scope.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->"
        }}),
    ]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output transcript");

    let meta = exact_test_make_review_meta(session_id, ReviewDecision::Pass, "CLEAN");
    review_cmd::persist_review_verdict_for_tests(project_dir.path(), &meta, &[], Vec::new());

    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
}

#[test]
fn final_iteration_pass_overrides_transient_fail_and_prose_unavailable() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project_dir = exact_test_setup_git_repo();
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let branch = "fix-1764-pass";
    let head_sha = csa_session::detect_git_head(project_dir.path()).expect("detect HEAD");
    let (session_id, session_dir) = exact_test_create_review_session(
        project_dir.path(),
        branch,
        &head_sha,
        "review: issue-1764 final pass",
    );

    let prior_fail = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "Verdict: FAIL\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "Prior iteration emitted an unstructured fail verdict.\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    let final_pass = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "Verdict: PASS\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "No blocking findings remain.\n",
        "Codegraph was unavailable in this worktree, so review used git diff/source inspection.\n\n",
        "```findings.toml\n",
        "findings = []\n",
        "```\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    let full_output = [
        exact_test_codex_agent_message(prior_fail),
        exact_test_codex_agent_message(final_pass),
    ]
    .join("\n");
    std::fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write transcript");
    csa_session::persist_structured_output(&session_dir, final_pass)
        .expect("persist final structured output");

    let mut meta = exact_test_make_review_meta(&session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.fix_attempted = false;
    meta.fix_rounds = 0;
    meta.review_iterations = 3;
    meta.exit_code = 1;

    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        Some(&session_id),
    );

    assert_eq!(persisted_exit_code, Some(0));
    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    let persisted_meta: ReviewSessionMeta =
        serde_json::from_str(&std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|count| *count == 0));
    assert_eq!(persisted_meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(persisted_meta.verdict, "CLEAN");
    assert_eq!(persisted_meta.exit_code, 0);
    assert_eq!(persisted_meta.review_iterations, 3);

    let wait_summary = crate::session_cmds_daemon::render_wait_result_summary(
        &session_dir,
        &session_id,
        &exact_test_wait_result(0, "Codegraph was unavailable in this worktree"),
    );
    assert!(wait_summary.contains("Review verdict: PASS"));
    assert!(!wait_summary.contains("Review verdict: UNAVAILABLE"));

    let found = review_cmd::check_review_verdict_for_target(
        project_dir.path(),
        branch,
        &head_sha,
        "range:main...HEAD",
        None,
        None,
    )
    .unwrap()
    .expect("check-verdict should accept the canonical final pass");
    assert_eq!(found.session_id, session_id);
}

include!("review_cmd_exact_tests_tail.rs");
