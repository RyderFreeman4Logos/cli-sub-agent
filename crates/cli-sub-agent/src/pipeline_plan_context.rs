//! Read-only TODO plan context formatter for prompt injection.
//!
//! The formatter intentionally reads only TODO metadata and `spec.toml`.
//! Reference files are not part of this context because they can contain
//! untrusted bulk content.

use std::path::Path;

use csa_todo::{
    CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoAttestationStatus,
    TodoManager, TodoPlan,
};
use tracing::{debug, warn};

const PLAN_CONTEXT_MAX_LINES: usize = 30;

/// Load a lightweight plan context for the current branch.
///
/// Returns `None` when branch detection fails, no TODO plan exists for the
/// branch, or TODO state cannot be read.
pub(crate) fn load_plan_context(project_root: &Path) -> Option<String> {
    let branch = detect_branch(project_root)?;
    debug!(branch = %branch, "Checking for plan context");

    let manager = TodoManager::new(project_root).ok()?;
    load_plan_context_for_branch(&manager, &branch)
}

fn load_plan_context_for_branch(manager: &TodoManager, branch: &str) -> Option<String> {
    let plan = match manager.find_by_branch(branch) {
        Ok(plans) => plans.into_iter().next()?,
        Err(error) => {
            warn!(branch, error = %error, "Failed to find TODO plan for branch");
            return None;
        }
    };

    match manager.verify_attestation(&plan.timestamp) {
        Ok(TodoAttestationStatus::Missing | TodoAttestationStatus::Valid) => {}
        Ok(TodoAttestationStatus::Mismatch { .. }) => {
            warn!(
                branch,
                plan = %plan.timestamp,
                "[PLAN TAMPERED] Plan content does not match stored attestation hash"
            );
            return None;
        }
        Err(error) => {
            warn!(
                branch,
                plan = %plan.timestamp,
                error = %error,
                "Failed to verify TODO plan attestation"
            );
            return None;
        }
    }

    let spec = match manager.load_spec(&plan.timestamp) {
        Ok(spec) => spec,
        Err(error) => {
            warn!(
                branch,
                plan = %plan.timestamp,
                error = %error,
                "Failed to load TODO plan spec"
            );
            None
        }
    };

    Some(format_plan_context(&plan, spec.as_ref()))
}

fn format_plan_context(plan: &TodoPlan, spec: Option<&SpecDocument>) -> String {
    let branch = plan.metadata.branch.as_deref().unwrap_or("(none)");
    let mut lines = vec![
        "<plan-context>".to_string(),
        format!("Plan: {}", escape_xml_text(&plan.metadata.title)),
        format!("Branch: {}", escape_xml_text(branch)),
        format!("Current phase: {}", plan.metadata.status),
        "Checklist / DONE WHEN criteria:".to_string(),
    ];

    match spec {
        Some(spec) if !spec.criteria.is_empty() => {
            lines.extend(spec.criteria.iter().map(format_criterion_checkbox));
        }
        Some(_) => lines.push("(spec.toml has no criteria)".to_string()),
        None => lines.push("(no spec.toml criteria found)".to_string()),
    }

    lines.push("</plan-context>".to_string());
    truncate_plan_context_lines(lines).join("\n")
}

fn format_criterion_checkbox(criterion: &SpecCriterion) -> String {
    let checked = match criterion.status {
        CriterionStatus::Verified => "x",
        CriterionStatus::Pending | CriterionStatus::Failed => " ",
    };
    let status_suffix = match criterion.status {
        CriterionStatus::Pending => "",
        CriterionStatus::Verified => " (verified)",
        CriterionStatus::Failed => " (failed)",
    };

    format!(
        "- [{checked}] [{}] {}: {}{}",
        criterion_kind_label(criterion.kind),
        escape_xml_text(&criterion.id),
        escape_xml_text(&criterion.description),
        status_suffix
    )
}

fn criterion_kind_label(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::Scenario => "scenario",
        CriterionKind::Property => "property",
        CriterionKind::Check => "check",
    }
}

fn truncate_plan_context_lines(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() <= PLAN_CONTEXT_MAX_LINES {
        return lines;
    }

    let keep_count = PLAN_CONTEXT_MAX_LINES - 2;
    let omitted_count = lines.len().saturating_sub(keep_count + 1);
    lines.truncate(keep_count);
    lines.push(format!(
        "<!-- truncated: omitted {omitted_count} line(s) to keep plan-context <= {PLAN_CONTEXT_MAX_LINES} lines -->"
    ));
    lines.push("</plan-context>".to_string());
    lines
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Auto-detect current branch via VCS abstraction (supports both git and jj).
fn detect_branch(project_root: &Path) -> Option<String> {
    let backend = csa_session::vcs_backends::create_vcs_backend(project_root);
    backend.current_branch(project_root).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_todo::{SpecCriterion, TodoStatus};
    use tempfile::tempdir;

    fn criterion(id: &str, status: CriterionStatus) -> SpecCriterion {
        SpecCriterion {
            kind: CriterionKind::Scenario,
            id: id.to_string(),
            description: format!("{id} must pass."),
            status,
        }
    }

    #[test]
    fn plan_context_renders_structure_and_spec_checkboxes() {
        let dir = tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        let plan = manager
            .create("Goal drift prevention", Some("feat/plan-context"))
            .expect("plan");
        manager
            .update_status(&plan.timestamp, TodoStatus::Implementing)
            .expect("status");
        let spec = SpecDocument {
            plan_ulid: plan.timestamp.clone(),
            summary: "Keep sessions aligned to the active plan.".to_string(),
            criteria: vec![
                criterion("scenario-pending", CriterionStatus::Pending),
                criterion("scenario-verified", CriterionStatus::Verified),
                criterion("scenario-failed", CriterionStatus::Failed),
            ],
            ..SpecDocument::default()
        };
        manager
            .save_spec(&plan.timestamp, &spec)
            .expect("save spec");

        let context = load_plan_context_for_branch(&manager, "feat/plan-context")
            .expect("context should render");

        assert!(context.starts_with("<plan-context>\n"));
        assert!(context.contains("Plan: Goal drift prevention\n"));
        assert!(context.contains("Branch: feat/plan-context\n"));
        assert!(context.contains("Current phase: implementing\n"));
        assert!(context.contains("Checklist / DONE WHEN criteria:\n"));
        assert!(context.contains("- [ ] [scenario] scenario-pending: scenario-pending must pass."));
        assert!(context.contains(
            "- [x] [scenario] scenario-verified: scenario-verified must pass. (verified)"
        ));
        assert!(
            context
                .contains("- [ ] [scenario] scenario-failed: scenario-failed must pass. (failed)")
        );
        assert!(context.ends_with("</plan-context>"));
        assert!(context.lines().count() <= PLAN_CONTEXT_MAX_LINES);
    }

    #[test]
    fn plan_context_truncates_long_specs_to_thirty_lines() {
        let dir = tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        let plan = manager
            .create("Long spec", Some("feat/long-spec"))
            .expect("plan");
        let spec = SpecDocument {
            plan_ulid: plan.timestamp.clone(),
            summary: "Long criteria list.".to_string(),
            criteria: (0..40)
                .map(|index| criterion(&format!("criterion-{index:02}"), CriterionStatus::Pending))
                .collect(),
            ..SpecDocument::default()
        };
        manager
            .save_spec(&plan.timestamp, &spec)
            .expect("save spec");

        let context = load_plan_context_for_branch(&manager, "feat/long-spec")
            .expect("context should render");

        assert_eq!(context.lines().count(), PLAN_CONTEXT_MAX_LINES);
        assert!(context.contains("Plan: Long spec\n"));
        assert!(context.contains("Branch: feat/long-spec\n"));
        assert!(context.contains("Current phase: draft\n"));
        assert!(context.contains("<!-- truncated: omitted"));
        assert!(context.ends_with("</plan-context>"));
    }

    #[test]
    fn plan_context_returns_none_when_branch_has_no_plan() {
        let dir = tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        assert!(load_plan_context_for_branch(&manager, "feat/missing").is_none());
    }

    #[test]
    fn plan_context_refuses_tampered_plan() {
        let dir = tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        let plan = manager
            .create("Protected plan", Some("feat/tamper"))
            .expect("plan");
        // Establish a real attestation baseline (what `csa todo save` does); only a
        // post-attestation edit is tamper. A freshly created, un-attested draft is
        // `Missing` and is loaded normally (#1669), not refused.
        manager.attest(&plan.timestamp).expect("attest");
        std::fs::write(plan.todo_md_path(), "# Tampered\n").expect("tamper");

        assert!(load_plan_context_for_branch(&manager, "feat/tamper").is_none());
    }
}
