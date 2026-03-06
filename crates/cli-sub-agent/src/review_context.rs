use std::path::Path;

use anyhow::{Context, Result};
use csa_todo::{CriterionKind, CriterionStatus, SpecDocument, TodoManager};

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
) -> Result<Option<ResolvedReviewContext>> {
    match requested_context {
        Some(path) => Ok(Some(resolve_explicit_review_context(path)?)),
        None => Ok(auto_discover_review_context(project_root)),
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
    discover_review_context_for_branch(&manager, &branch)
        .ok()
        .flatten()
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
