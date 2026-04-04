use std::path::Path;

use anyhow::{Context, Result};
use csa_todo::{CriterionKind, CriterionStatus, SpecDocument, TodoManager};
use tracing::warn;

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
