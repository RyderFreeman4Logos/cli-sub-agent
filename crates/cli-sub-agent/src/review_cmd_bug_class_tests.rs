use super::*;
use crate::bug_class::classify_recurring_bug_classes;
use crate::review_cmd::bug_class_pipeline::load_bug_class_review_artifacts;
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
    write_review_artifact_file(session_dir, "review-findings.json", artifact);
}

fn write_review_artifact_file(session_dir: &Path, file_name: &str, artifact: &ReviewArtifact) {
    let payload = serde_json::to_string_pretty(artifact).unwrap();
    std::fs::write(session_dir.join(file_name), payload).unwrap();
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

#[test]
fn review_iterations_use_max_prior_value_instead_of_most_recent_session() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    let older_high = create_session(
        project_dir.path(),
        Some("older review"),
        None,
        Some("codex"),
    )
    .expect("older session");
    let older_high_dir = get_session_dir(project_dir.path(), &older_high.meta_session_id).unwrap();
    write_review_meta(
        &older_high_dir,
        &ReviewSessionMeta {
            session_id: older_high.meta_session_id.clone(),
            head_sha: "deadbeef".to_string(),
            decision: "fail".to_string(),
            verdict: "HAS_ISSUES".to_string(),
            tool: "codex".to_string(),
            scope: "base:main".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 5,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        },
    )
    .expect("older review meta");

    let newer_low = create_session(
        project_dir.path(),
        Some("newer review"),
        None,
        Some("codex"),
    )
    .expect("newer session");
    let newer_low_dir = get_session_dir(project_dir.path(), &newer_low.meta_session_id).unwrap();
    write_review_meta(
        &newer_low_dir,
        &ReviewSessionMeta {
            session_id: newer_low.meta_session_id.clone(),
            head_sha: "deadbeef".to_string(),
            decision: "fail".to_string(),
            verdict: "HAS_ISSUES".to_string(),
            tool: "codex".to_string(),
            scope: "base:main".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 2,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        },
    )
    .expect("newer review meta");

    let current = create_session(
        project_dir.path(),
        Some("current review"),
        None,
        Some("codex"),
    )
    .expect("current session");

    let review_iterations =
        try_resolve_review_iterations(project_dir.path(), &current.meta_session_id).unwrap();
    assert_eq!(review_iterations, 6);
}

#[test]
fn bug_class_loader_collapses_multi_reviewer_sessions_into_one_logical_review() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    let reviewer_one = create_session(
        project_dir.path(),
        Some("review[1]: base:main"),
        None,
        Some("codex"),
    )
    .expect("reviewer one session");
    let reviewer_two = create_session(
        project_dir.path(),
        Some("review[2]: base:main"),
        None,
        Some("opencode"),
    )
    .expect("reviewer two session");
    let reviewer_one_dir =
        get_session_dir(project_dir.path(), &reviewer_one.meta_session_id).unwrap();
    let reviewer_two_dir =
        get_session_dir(project_dir.path(), &reviewer_two.meta_session_id).unwrap();

    write_review_artifact(
        &reviewer_one_dir,
        &sample_review_artifact(&reviewer_one.meta_session_id, Severity::High, "rust/002"),
    );
    write_review_artifact(
        &reviewer_two_dir,
        &sample_review_artifact(&reviewer_two.meta_session_id, Severity::High, "rust/003"),
    );

    for session in [&reviewer_one, &reviewer_two] {
        let session_dir = get_session_dir(project_dir.path(), &session.meta_session_id).unwrap();
        write_review_meta(
            &session_dir,
            &ReviewSessionMeta {
                session_id: session.meta_session_id.clone(),
                head_sha: "deadbeef".to_string(),
                decision: "fail".to_string(),
                verdict: "HAS_ISSUES".to_string(),
                tool: "codex".to_string(),
                scope: "base:main".to_string(),
                exit_code: 1,
                fix_attempted: false,
                fix_rounds: 0,
                review_iterations: 7,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: Some("sha256:shared".to_string()),
            },
        )
        .expect("review meta");
    }

    let review_artifacts = load_bug_class_review_artifacts(project_dir.path())
        .expect("multi-reviewer artifacts should collapse");

    assert_eq!(review_artifacts.len(), 1);
    assert_eq!(review_artifacts[0].findings.len(), 2);
    assert!(
        classify_recurring_bug_classes(&review_artifacts).is_empty(),
        "one logical review should not satisfy the recurrence threshold by itself"
    );
}

#[test]
fn bug_class_loader_skips_parent_consolidated_artifact_to_avoid_false_promotion() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    let parent = create_session(
        project_dir.path(),
        Some("review: base:main"),
        None,
        Some("codex"),
    )
    .expect("parent review session");
    let reviewer_one = create_session(
        project_dir.path(),
        Some("review[1]: base:main"),
        None,
        Some("codex"),
    )
    .expect("reviewer one session");
    let reviewer_two = create_session(
        project_dir.path(),
        Some("review[2]: base:main"),
        None,
        Some("opencode"),
    )
    .expect("reviewer two session");

    let parent_dir = get_session_dir(project_dir.path(), &parent.meta_session_id).unwrap();
    let reviewer_one_dir =
        get_session_dir(project_dir.path(), &reviewer_one.meta_session_id).unwrap();
    let reviewer_two_dir =
        get_session_dir(project_dir.path(), &reviewer_two.meta_session_id).unwrap();

    write_review_artifact_file(
        &parent_dir,
        "review-consolidated.json",
        &sample_review_artifact(&parent.meta_session_id, Severity::High, "rust/002"),
    );
    write_review_artifact(
        &reviewer_one_dir,
        &sample_review_artifact(&reviewer_one.meta_session_id, Severity::High, "rust/002"),
    );
    write_review_artifact(
        &reviewer_two_dir,
        &sample_review_artifact(&reviewer_two.meta_session_id, Severity::High, "rust/002"),
    );

    for session in [&reviewer_one, &reviewer_two] {
        let session_dir = get_session_dir(project_dir.path(), &session.meta_session_id).unwrap();
        write_review_meta(
            &session_dir,
            &ReviewSessionMeta {
                session_id: session.meta_session_id.clone(),
                head_sha: "deadbeef".to_string(),
                decision: "fail".to_string(),
                verdict: "HAS_ISSUES".to_string(),
                tool: "codex".to_string(),
                scope: "base:main".to_string(),
                exit_code: 1,
                fix_attempted: false,
                fix_rounds: 0,
                review_iterations: 7,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: Some("sha256:shared".to_string()),
            },
        )
        .expect("review meta");
    }

    let review_artifacts = load_bug_class_review_artifacts(project_dir.path())
        .expect("multi-reviewer artifacts should collapse");

    assert_eq!(review_artifacts.len(), 1);
    assert!(
        review_artifacts
            .iter()
            .all(|artifact| artifact.session_id != parent.meta_session_id),
        "parent consolidated artifact should be skipped for bug-class extraction"
    );
    assert!(
        classify_recurring_bug_classes(&review_artifacts).is_empty(),
        "one multi-review run must not self-promote a recurring bug class"
    );
}

#[test]
fn bug_class_loader_keeps_sessions_with_review_meta_even_if_consolidated_exists() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);

    let child = create_session(
        project_dir.path(),
        Some("review[1]: base:main"),
        None,
        Some("codex"),
    )
    .expect("child review session");
    let child_dir = get_session_dir(project_dir.path(), &child.meta_session_id).unwrap();

    write_review_artifact_file(
        &child_dir,
        "review-consolidated.json",
        &sample_review_artifact(&child.meta_session_id, Severity::High, "rust/002"),
    );
    write_review_meta(
        &child_dir,
        &ReviewSessionMeta {
            session_id: child.meta_session_id.clone(),
            head_sha: "deadbeef".to_string(),
            decision: "fail".to_string(),
            verdict: "HAS_ISSUES".to_string(),
            tool: "codex".to_string(),
            scope: "base:main".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 7,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: Some("sha256:shared".to_string()),
        },
    )
    .expect("review meta");

    let review_artifacts = load_bug_class_review_artifacts(project_dir.path())
        .expect("child session should still be mined");

    assert_eq!(review_artifacts.len(), 1);
    assert_eq!(review_artifacts[0].session_id, child.meta_session_id);
}
