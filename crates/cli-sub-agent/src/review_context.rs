use std::path::Path;

use anyhow::{Context, Result};
use csa_session::{Finding, ReviewArtifact, ReviewSessionMeta, Severity};
use csa_todo::{CriterionKind, CriterionStatus, SpecDocument, TodoManager};
use tracing::warn;

use crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedReviewContextKind {
    TodoMarkdown,
    Passthrough,
    SpecToml { spec: SpecDocument },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedReviewContext {
    pub(crate) path: String,
    pub(crate) kind: ResolvedReviewContextKind,
}

pub(crate) fn resolve_review_context(
    requested_context: Option<&str>,
    project_root: &Path,
    allow_auto_discovery: bool,
) -> Result<Option<ResolvedReviewContext>> {
    match requested_context {
        Some(path) => Ok(Some(resolve_explicit_review_context(path)?)),
        None if allow_auto_discovery => Ok(auto_discover_review_context(project_root)),
        None => Ok(None),
    }
}

fn resolve_explicit_review_context(path: &str) -> Result<ResolvedReviewContext> {
    let path_ref = Path::new(path);

    // Security: reject paths with null bytes (potential injection)
    anyhow::ensure!(
        !path.contains('\0'),
        "Spec path contains null byte: rejected for security"
    );

    // Accept .toml and .spec as spec document formats (both parsed as TOML)
    if has_extension(path_ref, "toml") || has_extension(path_ref, "spec") {
        return load_spec_review_context(path_ref);
    }

    let kind = if has_extension(path_ref, "md") {
        ResolvedReviewContextKind::TodoMarkdown
    } else {
        ResolvedReviewContextKind::Passthrough
    };

    Ok(ResolvedReviewContext {
        path: path.to_string(),
        kind,
    })
}

fn auto_discover_review_context(project_root: &Path) -> Option<ResolvedReviewContext> {
    let branch = current_git_branch(project_root)?;
    let manager = TodoManager::new(project_root).ok()?;
    auto_discover_review_context_for_branch(&manager, &branch)
}

fn auto_discover_review_context_for_branch(
    manager: &TodoManager,
    branch: &str,
) -> Option<ResolvedReviewContext> {
    match discover_review_context_for_branch(manager, branch) {
        Ok(context) => context,
        Err(error) => {
            warn!(
                branch,
                error = %error,
                "Failed to auto-discover review context"
            );
            None
        }
    }
}

pub(crate) fn discover_review_context_for_branch(
    manager: &TodoManager,
    branch: &str,
) -> Result<Option<ResolvedReviewContext>> {
    let Some(plan) = manager.find_by_branch(branch)?.into_iter().next() else {
        return Ok(None);
    };

    let spec_path = manager.spec_path(&plan.timestamp);
    if !spec_path.is_file() {
        return Ok(None);
    }

    load_spec_review_context(&spec_path).map(Some)
}

fn load_spec_review_context(path: &Path) -> Result<ResolvedReviewContext> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read spec context: {}", path.display()))?;
    let spec: SpecDocument = toml::from_str(&content)
        .with_context(|| format!("Failed to parse spec context: {}", path.display()))?;

    Ok(ResolvedReviewContext {
        path: path.display().to_string(),
        kind: ResolvedReviewContextKind::SpecToml { spec },
    })
}

fn current_git_branch(project_root: &Path) -> Option<String> {
    // Use VcsBackend for VCS-aware branch detection (supports both git and jj)
    let backend = csa_session::vcs_backends::create_vcs_backend(project_root);
    backend.current_branch(project_root).ok().flatten()
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

/// Maximum character length for review checklist content.
/// Files exceeding this are truncated with a warning.
const REVIEW_CHECKLIST_MAX_CHARS: usize = 4000;

/// Discover project-specific review checklist from `.csa/review-checklist.md`.
///
/// Returns `None` if the file does not exist or is empty.
/// Truncates content with a warning comment if it exceeds [`REVIEW_CHECKLIST_MAX_CHARS`].
pub(crate) fn discover_review_checklist(project_root: &Path) -> Option<String> {
    let checklist_path = project_root.join(".csa").join("review-checklist.md");
    let content = std::fs::read_to_string(&checklist_path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.len() > REVIEW_CHECKLIST_MAX_CHARS {
        // Find last valid char boundary at or before the max length to avoid
        // panicking on multi-byte UTF-8 characters (e.g. Chinese text).
        let safe_end = floor_char_boundary(trimmed, REVIEW_CHECKLIST_MAX_CHARS);
        let truncated = &trimmed[..safe_end];
        // Find last newline to avoid cutting mid-line
        let cut_point = truncated.rfind('\n').unwrap_or(safe_end);
        let mut result = trimmed[..cut_point].to_string();
        result.push_str("\n\n<!-- WARNING: review checklist truncated (exceeded 4000 chars) -->");
        warn!(
            path = %checklist_path.display(),
            original_len = trimmed.len(),
            "Review checklist truncated to {REVIEW_CHECKLIST_MAX_CHARS} chars"
        );
        Some(result)
    } else {
        Some(trimmed.to_string())
    }
}

/// Maximum prior-round findings injected into the current review prompt.
/// Caps prompt growth when a prior round produced many findings.
const MAX_PRIOR_ROUND_FINDINGS: usize = 12;

/// Summary of the prior cumulative review round, restricted to fields safe to
/// inject into a review prompt. Excludes raw diff text, file contents, env
/// vars, api keys, and user TOML.
pub(crate) struct PriorRoundSummary {
    pub(crate) session_id: String,
    pub(crate) decision: String,
    pub(crate) review_iterations: u32,
    pub(crate) findings: Vec<PriorRoundFinding>,
}

pub(crate) struct PriorRoundFinding {
    pub(crate) severity: String,
    pub(crate) file: String,
    pub(crate) line: Option<u32>,
    pub(crate) summary: String,
}

/// Discover the most recent prior review session on `branch` (excluding
/// `current_session_id`) and render a prompt-safe `## Prior-Round Assumptions
/// to Re-verify` section. Returns `None` when no prior round exists — i.e.
/// `review_iterations == 1` for the current run.
///
/// Whitelisted fields: `decision`, `review_iterations`, `findings[*].severity`,
/// `findings[*].file`, `findings[*].line`, `findings[*].summary`. Never reads
/// env vars, api keys, file contents, diff text, or user TOML.
pub(crate) fn discover_prior_round_assumptions(
    project_root: &Path,
    branch: Option<&str>,
    current_session_id: Option<&str>,
) -> Option<String> {
    let branch = branch?;
    let (prior_session_id, meta) =
        find_latest_branch_review_meta(project_root, branch, current_session_id)?;
    let findings = load_whitelisted_findings(project_root, &prior_session_id).unwrap_or_default();
    let summary = PriorRoundSummary {
        session_id: prior_session_id,
        decision: meta.decision,
        review_iterations: meta.review_iterations,
        findings,
    };
    Some(render_prior_round_assumptions(&summary))
}

fn find_latest_branch_review_meta(
    project_root: &Path,
    branch: &str,
    exclude_session_id: Option<&str>,
) -> Option<(String, ReviewSessionMeta)> {
    let sessions = csa_session::list_sessions(project_root, None).ok()?;
    sessions
        .into_iter()
        .filter(|candidate| candidate.resolved_identity().ref_name.as_deref() == Some(branch))
        .filter(|candidate| {
            exclude_session_id
                .map(|id| candidate.meta_session_id != id)
                .unwrap_or(true)
        })
        .filter_map(|candidate| {
            let meta = load_review_meta(project_root, &candidate.meta_session_id)?;
            Some((candidate.meta_session_id, meta))
        })
        .max_by(|a, b| {
            a.1.timestamp
                .cmp(&b.1.timestamp)
                .then_with(|| a.0.cmp(&b.0))
        })
}

fn load_review_meta(project_root: &Path, session_id: &str) -> Option<ReviewSessionMeta> {
    let session_dir = csa_session::get_session_dir(project_root, session_id).ok()?;
    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&review_meta_path).ok()?;
    serde_json::from_str::<ReviewSessionMeta>(&content).ok()
}

fn load_whitelisted_findings(
    project_root: &Path,
    session_id: &str,
) -> Option<Vec<PriorRoundFinding>> {
    let session_dir = csa_session::get_session_dir(project_root, session_id).ok()?;
    for name in [CONSOLIDATED_REVIEW_ARTIFACT_FILE, "review-findings.json"] {
        let path = session_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        let artifact: ReviewArtifact = serde_json::from_str(&content).ok()?;
        return Some(
            artifact
                .findings
                .into_iter()
                .take(MAX_PRIOR_ROUND_FINDINGS)
                .map(finding_to_prior_round)
                .collect(),
        );
    }
    None
}

fn finding_to_prior_round(finding: Finding) -> PriorRoundFinding {
    PriorRoundFinding {
        severity: severity_label(&finding.severity).to_string(),
        file: finding.file,
        line: finding.line,
        summary: finding.summary,
    }
}

fn severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
    }
}

fn render_prior_round_assumptions(summary: &PriorRoundSummary) -> String {
    let mut out = String::from("\n\n## Prior-Round Assumptions to Re-verify\n");
    out.push_str(&format!(
        "Prior review session `{}` (iteration {}) reached decision `{}`.\n",
        summary.session_id, summary.review_iterations, summary.decision
    ));
    if summary.findings.is_empty() {
        out.push_str(
            "No structured findings captured. Re-verify the prior verdict still holds for the current diff.\n",
        );
    } else {
        out.push_str("Re-verify each prior-round assumption still holds for the current diff:\n");
        for f in &summary.findings {
            let line_suffix = f.line.map(|l| format!(":{l}")).unwrap_or_default();
            out.push_str(&format!(
                "- [{}] {}{} -- {}\n",
                f.severity, f.file, line_suffix, f.summary
            ));
        }
    }
    out
}

pub(crate) fn render_spec_review_context(spec: &SpecDocument) -> String {
    let mut rendered = String::new();
    rendered.push_str(&format!("Plan ULID: {}\n", spec.plan_ulid));
    rendered.push_str(&format!("Summary: {}\n", spec.summary));
    rendered.push_str("Criteria:\n");
    for criterion in &spec.criteria {
        rendered.push_str(&format!(
            "- [{}] {} {}: {}\n",
            criterion_status_label(criterion.status),
            criterion_kind_label(criterion.kind),
            criterion.id,
            criterion.description
        ));
    }
    rendered
}

/// Find the last valid UTF-8 char boundary at or before `max_bytes`.
///
/// On stable Rust this replaces `str::floor_char_boundary()` (nightly-only).
/// Prevents panics when truncating strings containing multi-byte characters
/// (e.g. Chinese text common in this project).
fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn criterion_kind_label(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::Scenario => "scenario",
        CriterionKind::Property => "property",
        CriterionKind::Check => "check",
    }
}

fn criterion_status_label(status: CriterionStatus) -> &'static str {
    match status {
        CriterionStatus::Pending => "pending",
        CriterionStatus::Verified => "verified",
        CriterionStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Build a multi-byte UTF-8 string for testing (avoids literal CJK in source).
    fn multibyte_line() -> String {
        // U+4F60 U+597D = 2 CJK chars, 6 bytes
        format!("- [ ] {}{}\n", '\u{4F60}', '\u{597D}')
    }

    /// Build a short multi-byte string: 4 CJK chars = 12 bytes.
    fn multibyte_short() -> String {
        format!("{}{}{}{}", '\u{4F60}', '\u{597D}', '\u{4E16}', '\u{754C}')
    }

    #[test]
    fn discover_review_checklist_truncates_multibyte_text_without_panic() {
        let temp = tempdir().unwrap();
        let csa_dir = temp.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();

        // Build content with multi-byte chars (3 bytes each in UTF-8) that
        // exceeds REVIEW_CHECKLIST_MAX_CHARS.  Truncating by byte index without
        // respecting char boundaries would panic.
        let line = multibyte_line();
        let repeat_count = (REVIEW_CHECKLIST_MAX_CHARS / line.len()) + 10;
        let oversized = line.repeat(repeat_count);
        assert!(oversized.len() > REVIEW_CHECKLIST_MAX_CHARS);

        std::fs::write(csa_dir.join("review-checklist.md"), &oversized).unwrap();

        // Must not panic
        let checklist = discover_review_checklist(temp.path()).unwrap();
        assert!(checklist.contains("WARNING: review checklist truncated"));
        // Verify the output is valid UTF-8 (String guarantees this, but let's
        // also confirm it doesn't end mid-character).
        assert!(checklist.is_char_boundary(checklist.len()));
    }

    #[test]
    fn floor_char_boundary_on_ascii() {
        assert_eq!(super::floor_char_boundary("hello", 3), 3);
        assert_eq!(super::floor_char_boundary("hello", 10), 5);
    }

    #[test]
    fn floor_char_boundary_on_multibyte() {
        let s = multibyte_short(); // 4 chars x 3 bytes = 12 bytes
        // Byte 4 is mid-char; should snap back to byte 3.
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
        let _sandbox = ScopedSessionSandbox::new(&project);

        let result = discover_prior_round_assumptions(project.path(), Some("feat/iter1"), None);
        assert!(
            result.is_none(),
            "iter=1 (no prior session) must not inject Prior-Round section"
        );
    }

    #[test]
    fn prior_round_assumptions_render_when_prior_exists() {
        use crate::test_session_sandbox::ScopedSessionSandbox;
        use csa_session::{create_session, get_session_dir, write_review_meta};

        let project = setup_git_repo_with_branch("feat/iter2");
        let _sandbox = ScopedSessionSandbox::new(&project);

        let prior = create_session(project.path(), Some("prior review"), None, Some("codex"))
            .expect("prior session created");
        let prior_dir = get_session_dir(project.path(), &prior.meta_session_id).unwrap();

        write_review_meta(
            &prior_dir,
            &make_review_meta(&prior.meta_session_id, "fail", 1),
        )
        .expect("write review meta");

        let artifact = make_review_artifact(&prior.meta_session_id, Severity::High);
        std::fs::write(
            prior_dir.join("review-findings.json"),
            serde_json::to_string(&artifact).unwrap(),
        )
        .expect("write findings");

        let result = discover_prior_round_assumptions(project.path(), Some("feat/iter2"), None);
        let rendered = result.expect("iter=2 must inject Prior-Round section");

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
        let _sandbox = ScopedSessionSandbox::new(&project);
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

        // Generate content exceeding REVIEW_CHECKLIST_MAX_CHARS (4000)
        let line = "- [ ] Check this important review item number N\n";
        let repeat_count = (REVIEW_CHECKLIST_MAX_CHARS / line.len()) + 10;
        let oversized = line.repeat(repeat_count);
        assert!(oversized.len() > REVIEW_CHECKLIST_MAX_CHARS);

        std::fs::write(csa_dir.join("review-checklist.md"), &oversized).unwrap();

        let checklist = discover_review_checklist(temp.path()).unwrap();

        assert!(checklist.len() < oversized.trim().len());
        assert!(checklist.contains("WARNING: review checklist truncated"));
        // Content should be cut at a newline boundary
        assert!(!checklist.starts_with('\n'));
    }
}
