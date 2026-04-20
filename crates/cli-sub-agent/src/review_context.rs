use std::path::Path;

use anyhow::{Context, Result};
use csa_session::{Finding, ReviewArtifact, ReviewSessionMeta, Severity};
use csa_todo::{CriterionKind, CriterionStatus, SpecDocument, TodoManager};
use tracing::warn;

use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};

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
    let path = [
        session_dir.join(CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        session_dir.join(SINGLE_REVIEW_ARTIFACT_FILE),
    ]
    .into_iter()
    .find(|path| path.is_file())?;
    let content = std::fs::read_to_string(&path).ok()?;
    let artifact: ReviewArtifact = serde_json::from_str(&content).ok()?;
    Some(
        artifact
            .findings
            .into_iter()
            .take(MAX_PRIOR_ROUND_FINDINGS)
            .map(finding_to_prior_round)
            .collect(),
    )
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
#[path = "review_context_tests.rs"]
mod tests;
