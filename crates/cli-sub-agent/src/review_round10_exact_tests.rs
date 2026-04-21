use std::path::PathBuf;

fn review_round10_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn review_round10_run_git_cmd(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
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

fn review_round10_setup_git_repo_with_branch(branch: &str) -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("tempdir");
    review_round10_run_git_cmd(temp.path(), &["init", "--initial-branch", branch]);
    review_round10_run_git_cmd(temp.path(), &["config", "user.email", "test@example.com"]);
    review_round10_run_git_cmd(temp.path(), &["config", "user.name", "Test"]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").expect("write seed");
    review_round10_run_git_cmd(temp.path(), &["add", "seed.txt"]);
    review_round10_run_git_cmd(temp.path(), &["commit", "-m", "init"]);
    temp
}

#[test]
fn reviewer_prompt_documents_unavailable_state() {
    for relative_path in [
        "patterns/csa-review/PATTERN.md",
        "patterns/csa-review/workflow.toml",
        "patterns/csa-review/skills/csa-review/SKILL.md",
        "patterns/csa-review/skills/csa-review/references/output-schema.md",
    ] {
        let content = std::fs::read_to_string(review_round10_workspace_root().join(relative_path))
            .expect("read review contract doc");
        let lower = content.to_ascii_lowercase();
        assert!(
            lower.contains("unavailable"),
            "{relative_path} must mention the unavailable decision state"
        );
    }

    let pattern =
        std::fs::read_to_string(review_round10_workspace_root().join("patterns/csa-review/PATTERN.md"))
            .expect("read csa-review pattern");
    let lower = pattern.to_ascii_lowercase();
    assert!(lower.contains("quota/auth/network"));
    assert!(lower.contains("lacks confidence"));
}

#[test]
fn recurring_bug_extraction_prefers_session_findings_over_root() {
    use crate::test_env_lock::ScopedEnvVarRestore;
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_session::review_artifact::{Finding, FindingsFile, ReviewArtifact, Severity, SeveritySummary};
    use csa_session::{create_session, get_session_dir, write_findings_toml};
    use tempfile::tempdir;

    fn artifact(session_id: &str, severity: Severity, rule_id: &str) -> ReviewArtifact {
        let findings = vec![Finding {
            severity,
            fid: format!("FID-{session_id}"),
            file: "src/lib.rs".to_string(),
            line: Some(17),
            rule_id: rule_id.to_string(),
            summary: "Avoid unwrap in library code.".to_string(),
            engine: "reviewer".to_string(),
        }];
        ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings,
            review_mode: Some("single".to_string()),
            schema_version: "1.0".to_string(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    let temp = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
    let config_home = temp.path().join("config");
    std::fs::create_dir_all(&config_home).expect("create config home");
    let _config_guard = ScopedEnvVarRestore::set(
        "XDG_CONFIG_HOME",
        config_home.to_str().expect("config home utf-8"),
    );
    let _home_guard =
        ScopedEnvVarRestore::set("HOME", temp.path().to_str().expect("home path utf-8"));
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("create project root");

    let previous = create_session(&project_root, Some("previous review"), None, Some("codex"))
        .expect("create previous review session");
    let current = create_session(&project_root, Some("current review"), None, Some("codex"))
        .expect("create current review session");
    let previous_dir = get_session_dir(&project_root, &previous.meta_session_id)
        .expect("resolve previous session dir");
    let current_dir = get_session_dir(&project_root, &current.meta_session_id)
        .expect("resolve current session dir");

    std::fs::write(
        previous_dir.join("review-findings.json"),
        serde_json::to_string_pretty(&artifact(
            &previous.meta_session_id,
            Severity::High,
            "rust/002",
        ))
        .expect("serialize previous artifact"),
    )
    .expect("write previous artifact");
    std::fs::write(
        current_dir.join("review-findings.json"),
        serde_json::to_string_pretty(&artifact(
            &current.meta_session_id,
            Severity::High,
            "rust/stale",
        ))
        .expect("serialize stale current artifact"),
    )
    .expect("write stale current artifact");
    write_findings_toml(
        &current_dir,
        &FindingsFile {
            findings: Vec::new(),
        },
    )
    .expect("write empty current findings.toml");

    crate::review_cmd::try_extract_recurring_bug_class_skills(
        &project_root,
        std::slice::from_ref(&current.meta_session_id),
    )
    .expect("recurring bug extraction should succeed");

    let skill_dir = csa_config::paths::config_dir_write()
        .expect("resolve config dir")
        .join("skills/code-quality-rust");
    assert!(
        !skill_dir.exists(),
        "stale root findings must not generate recurring skills when findings.toml is empty"
    );
}

#[test]
fn prior_round_context_prefers_session_findings_over_root() {
    use crate::review_context::discover_prior_round_assumptions;
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_session::review_artifact::{Finding, FindingsFile, ReviewArtifact, Severity, SeveritySummary};
    use csa_session::state::ReviewSessionMeta;
    use csa_session::{create_session, get_session_dir, write_findings_toml, write_review_meta};

    fn review_meta(session_id: &str) -> ReviewSessionMeta {
        ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "deadbeef".to_string(),
            decision: "fail".to_string(),
            verdict: "HAS_ISSUES".to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "base:main".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        }
    }

    fn review_artifact(session_id: &str) -> ReviewArtifact {
        let findings = vec![Finding {
            severity: Severity::High,
            fid: "F-001".to_string(),
            file: "src/lib.rs".to_string(),
            line: Some(42),
            rule_id: "rust/test".to_string(),
            summary: "Assumption no unwrap in production path".to_string(),
            engine: "reviewer".to_string(),
        }];
        ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings,
            review_mode: None,
            schema_version: "1.0".to_string(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    let project = review_round10_setup_git_repo_with_branch(
        "feat/prior-round-session-findings-exact",
    );
    let _sandbox = ScopedSessionSandbox::new_blocking(&project);

    let prior = create_session(project.path(), Some("prior review"), None, Some("codex"))
        .expect("prior session created");
    let prior_dir =
        get_session_dir(project.path(), &prior.meta_session_id).expect("resolve prior dir");

    write_review_meta(&prior_dir, &review_meta(&prior.meta_session_id)).expect("write review meta");
    std::fs::write(
        prior_dir.join("review-findings.json"),
        serde_json::to_string(&review_artifact(&prior.meta_session_id))
            .expect("serialize stale root artifact"),
    )
    .expect("write stale root artifact");
    write_findings_toml(
        &prior_dir,
        &FindingsFile {
            findings: Vec::new(),
        },
    )
    .expect("write empty findings.toml");

    let rendered = discover_prior_round_assumptions(
        project.path(),
        Some("feat/prior-round-session-findings-exact"),
        None,
    )
    .expect("prior round context should render");

    assert!(rendered.contains("No structured findings captured"));
    assert!(!rendered.contains("[high]"));
}
