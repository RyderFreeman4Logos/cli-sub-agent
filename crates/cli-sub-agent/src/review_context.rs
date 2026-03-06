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
    if has_extension(path_ref, "toml") {
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
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
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
}
