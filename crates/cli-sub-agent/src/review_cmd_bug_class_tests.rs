use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
use csa_session::state::ReviewSessionMeta;
use csa_session::{create_session, get_session_dir, write_review_meta};
use std::path::Path;
use tempfile::tempdir;

fn sample_review_artifact(session_id: &str, severity: Severity, rule_id: &str) -> ReviewArtifact {
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

fn write_review_artifact(session_dir: &Path, artifact: &ReviewArtifact) {
    let payload = serde_json::to_string_pretty(artifact).unwrap();
    std::fs::write(session_dir.join("review-findings.json"), payload).unwrap();
}

#[test]
fn recurring_bug_class_skill_extraction_runs_for_high_severity_review_completion() {
    let temp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp);
    let config_home = temp.path().join("config");
    std::fs::create_dir_all(&config_home).unwrap();
    let _config_guard = ScopedEnvVarRestore::set("XDG_CONFIG_HOME", config_home.to_str().unwrap());
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();

    let previous = create_session(&project_root, Some("previous review"), None, Some("codex"))
        .expect("previous session");
    let current = create_session(&project_root, Some("current review"), None, Some("codex"))
        .expect("current session");
    let previous_dir = get_session_dir(&project_root, &previous.meta_session_id).unwrap();
    let current_dir = get_session_dir(&project_root, &current.meta_session_id).unwrap();

    write_review_artifact(
        &previous_dir,
        &sample_review_artifact(&previous.meta_session_id, Severity::High, "rust/002"),
    );
    write_review_artifact(
        &current_dir,
        &sample_review_artifact(&current.meta_session_id, Severity::Critical, "rust/002"),
    );

    try_extract_recurring_bug_class_skills(
        &project_root,
        std::slice::from_ref(&current.meta_session_id),
    )
    .expect("skill extraction should succeed");

    let skill_dir = config_home.join("cli-sub-agent/skills/code-quality-rust");
    assert!(skill_dir.join("SKILL.md").is_file());
    assert!(
        std::fs::read_to_string(skill_dir.join("references/detailed-patterns.md"))
            .unwrap()
            .contains("Recurrence: 2 review session(s)")
    );
}

#[test]
fn review_iterations_increment_from_prior_review_meta_on_same_branch() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    let previous = create_session(
        project_dir.path(),
        Some("previous review"),
        None,
        Some("codex"),
    )
    .expect("previous session");
    let current = create_session(
        project_dir.path(),
        Some("current review"),
        None,
        Some("codex"),
    )
    .expect("current session");
    let previous_dir = get_session_dir(project_dir.path(), &previous.meta_session_id).unwrap();

    write_review_meta(
        &previous_dir,
        &ReviewSessionMeta {
            session_id: previous.meta_session_id.clone(),
            head_sha: "deadbeef".to_string(),
            decision: "fail".to_string(),
            verdict: "HAS_ISSUES".to_string(),
            tool: "codex".to_string(),
            scope: "base:main".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        },
    )
    .expect("review meta");

    let review_iterations =
        try_resolve_review_iterations(project_dir.path(), &current.meta_session_id).unwrap();
    assert_eq!(review_iterations, 2);
}

#[test]
fn review_iterations_do_not_undercount_after_more_than_ten_prior_reviews() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    for iteration in 1..=11 {
        let session = create_session(
            project_dir.path(),
            Some("previous review"),
            None,
            Some("codex"),
        )
        .expect("previous session");
        let session_dir = get_session_dir(project_dir.path(), &session.meta_session_id).unwrap();

        write_review_meta(
            &session_dir,
            &ReviewSessionMeta {
                session_id: session.meta_session_id,
                head_sha: "deadbeef".to_string(),
                decision: "fail".to_string(),
                verdict: "HAS_ISSUES".to_string(),
                tool: "codex".to_string(),
                scope: "base:main".to_string(),
                exit_code: 1,
                fix_attempted: false,
                fix_rounds: 0,
                review_iterations: iteration,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: None,
            },
        )
        .expect("review meta");
    }

    let current = create_session(
        project_dir.path(),
        Some("current review"),
        None,
        Some("codex"),
    )
    .expect("current session");

    let review_iterations =
        try_resolve_review_iterations(project_dir.path(), &current.meta_session_id).unwrap();
    assert_eq!(review_iterations, 12);
}
