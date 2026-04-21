use super::*;
use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};
use csa_todo::{SpecCriterion, TodoManager};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedLogBuffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}

struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn sample_spec_document(plan_ulid: &str, criterion_id: &str) -> SpecDocument {
    SpecDocument {
        schema_version: 1,
        plan_ulid: plan_ulid.to_string(),
        summary: format!("Spec summary for {plan_ulid}"),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Scenario,
            id: criterion_id.to_string(),
            description: format!("Criterion {criterion_id} must be satisfied."),
            status: CriterionStatus::Pending,
        }],
    }
}

#[test]
fn resolve_review_context_skips_auto_discovery_when_disabled() {
    let temp = tempdir().unwrap();

    let context = resolve_review_context(None, temp.path(), false).unwrap();

    assert!(context.is_none());
}

#[test]
fn auto_discover_review_context_warns_on_invalid_spec() {
    let temp = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(temp.path().to_path_buf());
    let plan = manager
        .create("Broken Spec", Some("feat/broken-spec"))
        .unwrap();
    std::fs::write(manager.spec_path(&plan.timestamp), "not = [valid toml").unwrap();

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let context = auto_discover_review_context_for_branch(&manager, "feat/broken-spec");

    assert!(context.is_none());
    let output = buffer.contents();
    assert!(output.contains("Failed to auto-discover review context"));
    assert!(output.contains("Failed to parse spec context"));
    assert!(output.contains("feat/broken-spec"));
}

#[test]
fn resolve_review_context_accepts_dot_spec_extension() {
    let temp = tempdir().unwrap();
    let spec_path = temp.path().join("contract.spec");
    std::fs::write(
        &spec_path,
        toml::to_string_pretty(&sample_spec_document(
            "01JTESTPLAN0000000000000001",
            "criterion-spec-ext",
        ))
        .unwrap(),
    )
    .unwrap();

    let context = resolve_review_context(Some(spec_path.to_str().unwrap()), temp.path(), false)
        .unwrap()
        .unwrap();

    assert_eq!(context.path, spec_path.display().to_string());
    assert!(matches!(
        context.kind,
        ResolvedReviewContextKind::SpecToml { .. }
    ));
}

#[test]
fn resolve_review_context_explicit_spec_still_parses_when_auto_discovery_disabled() {
    let temp = tempdir().unwrap();
    let spec_path = temp.path().join("spec.toml");
    std::fs::write(
        &spec_path,
        toml::to_string_pretty(&sample_spec_document(
            "01JTESTPLAN0000000000000000",
            "criterion-login",
        ))
        .unwrap(),
    )
    .unwrap();

    let context = resolve_review_context(Some(spec_path.to_str().unwrap()), temp.path(), false)
        .unwrap()
        .unwrap();

    assert_eq!(context.path, spec_path.display().to_string());
    assert!(matches!(
        context.kind,
        ResolvedReviewContextKind::SpecToml { .. }
    ));
}

#[test]
fn discover_review_checklist_returns_content_when_file_exists() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("review-checklist.md"),
        "# Checklist\n- [ ] Check item one\n",
    )
    .unwrap();

    let checklist = discover_review_checklist(temp.path());

    assert!(checklist.is_some());
    let content = checklist.unwrap();
    assert!(content.contains("# Checklist"));
    assert!(content.contains("Check item one"));
}

#[test]
fn discover_review_checklist_returns_none_when_file_missing() {
    let temp = tempdir().unwrap();

    let checklist = discover_review_checklist(temp.path());

    assert!(checklist.is_none());
}

#[test]
fn discover_review_checklist_returns_none_for_empty_file() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(csa_dir.join("review-checklist.md"), "   \n\n  ").unwrap();

    let checklist = discover_review_checklist(temp.path());

    assert!(checklist.is_none());
}

fn multibyte_line() -> String {
    format!("- [ ] {}{}\n", '\u{4F60}', '\u{597D}')
}

fn multibyte_short() -> String {
    format!("{}{}{}{}", '\u{4F60}', '\u{597D}', '\u{4E16}', '\u{754C}')
}

#[test]
fn discover_review_checklist_truncates_multibyte_text_without_panic() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    let line = multibyte_line();
    let repeat_count = (REVIEW_CHECKLIST_MAX_CHARS / line.len()) + 10;
    let oversized = line.repeat(repeat_count);
    assert!(oversized.len() > REVIEW_CHECKLIST_MAX_CHARS);

    std::fs::write(csa_dir.join("review-checklist.md"), &oversized).unwrap();

    let checklist = discover_review_checklist(temp.path()).unwrap();
    assert!(checklist.contains("WARNING: review checklist truncated"));
    assert!(checklist.is_char_boundary(checklist.len()));
}

#[test]
fn floor_char_boundary_on_ascii() {
    assert_eq!(super::floor_char_boundary("hello", 3), 3);
    assert_eq!(super::floor_char_boundary("hello", 10), 5);
}

#[test]
fn floor_char_boundary_on_multibyte() {
    let s = multibyte_short();
    assert_eq!(super::floor_char_boundary(&s, 4), 3);
    assert_eq!(super::floor_char_boundary(&s, 5), 3);
    assert_eq!(super::floor_char_boundary(&s, 6), 6);
}

fn run_git_cmd(dir: &std::path::Path, args: &[&str]) {
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

fn setup_git_repo_with_branch(branch: &str) -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("tempdir");
    run_git_cmd(temp.path(), &["init", "--initial-branch", branch]);
    run_git_cmd(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git_cmd(temp.path(), &["config", "user.name", "Test"]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    run_git_cmd(temp.path(), &["add", "seed.txt"]);
    run_git_cmd(temp.path(), &["commit", "-m", "init"]);
    temp
}

fn make_review_meta(session_id: &str, decision: &str, iters: u32) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: "abc123".to_string(),
        decision: decision.to_string(),
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
        review_iterations: iters,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    }
}

fn make_review_artifact(session_id: &str, sev: Severity) -> ReviewArtifact {
    use csa_session::SeveritySummary;

    let findings = vec![Finding {
        severity: sev,
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

#[test]
fn prior_round_assumptions_none_when_no_prior_session() {
    use crate::test_session_sandbox::ScopedSessionSandbox;

    let project = setup_git_repo_with_branch("feat/iter1");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project);

    let result = discover_prior_round_assumptions(project.path(), Some("feat/iter1"), None);
    assert!(
        result.is_none(),
        "iter=1 (no prior session) must not inject Prior-Round section"
    );
}

#[test]
fn prior_round_assumptions_render_when_prior_consolidated_artifact_exists() {
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_session::{create_session, get_session_dir, write_review_meta};
    use std::fs;

    let project = setup_git_repo_with_branch("feat/iter2");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project);

    let prior = create_session(project.path(), Some("prior review"), None, Some("codex"))
        .expect("prior session created");
    let prior_dir = get_session_dir(project.path(), &prior.meta_session_id).unwrap();

    write_review_meta(
        &prior_dir,
        &make_review_meta(&prior.meta_session_id, "fail", 1),
    )
    .expect("write review meta");

    let reviewer_one = make_review_artifact("reviewer-1", Severity::High);
    let reviewer_two = ReviewArtifact {
        findings: vec![Finding {
            severity: Severity::Medium,
            fid: "F-002".to_string(),
            file: "src/review.rs".to_string(),
            line: Some(7),
            rule_id: "rust/test".to_string(),
            summary: "Second reviewer found a medium-severity assumption".to_string(),
            engine: "reviewer".to_string(),
        }],
        severity_summary: csa_session::SeveritySummary {
            critical: 0,
            high: 0,
            medium: 1,
            low: 0,
        },
        review_mode: Some("range:main...HEAD".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "reviewer-2".to_string(),
        timestamp: chrono::Utc::now(),
    };
    let consolidated = crate::review_consensus::build_consolidated_artifact(
        vec![reviewer_one, reviewer_two],
        &prior.meta_session_id,
    );
    fs::write(
        prior_dir.join(CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        serde_json::to_string(&consolidated).unwrap(),
    )
    .expect("write consolidated findings");

    let result = discover_prior_round_assumptions(project.path(), Some("feat/iter2"), None);
    let rendered = result.expect("iter=2 must inject Prior-Round section");

    assert!(rendered.contains("## Prior-Round Assumptions to Re-verify"));
    assert!(rendered.contains("iteration 1"));
    assert!(rendered.contains("decision `fail`"));
    assert!(rendered.contains("[high]"));
    assert!(rendered.contains("[medium]"));
    assert!(rendered.contains("src/lib.rs:42"));
    assert!(rendered.contains("src/review.rs:7"));
    assert!(rendered.contains("no unwrap"));
    assert!(rendered.contains("Second reviewer found a medium-severity assumption"));
}

#[test]
fn prior_round_assumptions_render_when_only_single_reviewer_artifact_exists() {
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_session::{create_session, get_session_dir, write_review_meta};
    use std::fs;

    let project = setup_git_repo_with_branch("feat/iter1-single");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project);

    let prior = create_session(project.path(), Some("prior review"), None, Some("codex"))
        .expect("prior session created");
    let prior_dir = get_session_dir(project.path(), &prior.meta_session_id).unwrap();

    write_review_meta(
        &prior_dir,
        &make_review_meta(&prior.meta_session_id, "fail", 1),
    )
    .expect("write review meta");

    fs::write(
        prior_dir.join(SINGLE_REVIEW_ARTIFACT_FILE),
        serde_json::to_string(&make_review_artifact("reviewer-1", Severity::High)).unwrap(),
    )
    .expect("write single-reviewer findings");
    assert!(
        !prior_dir.join(CONSOLIDATED_REVIEW_ARTIFACT_FILE).exists(),
        "single-reviewer regression test must exercise fallback without consolidated artifact"
    );

    let result = discover_prior_round_assumptions(project.path(), Some("feat/iter1-single"), None);
    let rendered = result.expect("iter=1 prior review must inject Prior-Round section");

    assert!(rendered.contains("## Prior-Round Assumptions to Re-verify"));
    assert!(rendered.contains("iteration 1"));
    assert!(rendered.contains("decision `fail`"));
    assert!(rendered.contains("[high]"));
    assert!(rendered.contains("src/lib.rs:42"));
    assert!(rendered.contains("no unwrap"));
}

#[test]
fn find_latest_branch_review_meta_breaks_timestamp_ties_by_session_id() {
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_session::{create_session, get_session_dir, write_review_meta};

    let project = setup_git_repo_with_branch("feat/tie-break");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project);
    let shared_timestamp = chrono::Utc::now();

    let session_a = create_session(project.path(), Some("review-a"), None, Some("codex"))
        .expect("session a created");
    let session_b = create_session(project.path(), Some("review-b"), None, Some("codex"))
        .expect("session b created");

    let session_a_dir = get_session_dir(project.path(), &session_a.meta_session_id).unwrap();
    let session_b_dir = get_session_dir(project.path(), &session_b.meta_session_id).unwrap();

    let mut meta_a = make_review_meta(&session_a.meta_session_id, "fail", 1);
    meta_a.timestamp = shared_timestamp;
    let mut meta_b = make_review_meta(&session_b.meta_session_id, "pass", 2);
    meta_b.timestamp = shared_timestamp;

    write_review_meta(&session_a_dir, &meta_a).expect("write session a review meta");
    write_review_meta(&session_b_dir, &meta_b).expect("write session b review meta");

    let expected_session_id = std::cmp::max(
        session_a.meta_session_id.clone(),
        session_b.meta_session_id.clone(),
    );

    for _ in 0..5 {
        let (selected_session_id, selected_meta) =
            find_latest_branch_review_meta(project.path(), "feat/tie-break", None)
                .expect("latest prior review session");
        assert_eq!(selected_session_id, expected_session_id);
        assert_eq!(selected_meta.session_id, expected_session_id);
        assert_eq!(selected_meta.timestamp, shared_timestamp);
    }
}

#[test]
fn render_prior_round_assumptions_handles_empty_findings() {
    let summary = PriorRoundSummary {
        session_id: "01TESTTIMESTAMP0001".to_string(),
        decision: "pass".to_string(),
        review_iterations: 3,
        findings: vec![],
    };
    let rendered = render_prior_round_assumptions(&summary);
    assert!(rendered.contains("## Prior-Round Assumptions to Re-verify"));
    assert!(rendered.contains("01TESTTIMESTAMP0001"));
    assert!(rendered.contains("iteration 3"));
    assert!(rendered.contains("decision `pass`"));
    assert!(rendered.contains("No structured findings captured"));
}

#[test]
fn discover_review_checklist_truncates_oversized_content() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    let line = "- [ ] Check this important review item number N\n";
    let repeat_count = (REVIEW_CHECKLIST_MAX_CHARS / line.len()) + 10;
    let oversized = line.repeat(repeat_count);
    assert!(oversized.len() > REVIEW_CHECKLIST_MAX_CHARS);

    std::fs::write(csa_dir.join("review-checklist.md"), &oversized).unwrap();

    let checklist = discover_review_checklist(temp.path()).unwrap();

    assert!(checklist.len() < oversized.trim().len());
    assert!(checklist.contains("WARNING: review checklist truncated"));
    assert!(!checklist.starts_with('\n'));
}
