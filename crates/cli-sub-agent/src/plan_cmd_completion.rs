use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::StepResult;
use super::plan_cmd_failure::PlanFailureError;

#[path = "plan_cmd_pr_bot_completion.rs"]
mod pr_bot_completion;
pub(crate) use pr_bot_completion::{
    PrBotFailureSideEffectInput, verify_pr_bot_failure_side_effects,
};

#[path = "plan_cmd_dev2merge_completion.rs"]
mod dev2merge_completion;
#[cfg(test)]
use dev2merge_completion::{
    Dev2MergeCompletionFacts, Dev2MergeObservedState, dev2merge_success_summary,
    evaluate_dev2merge_completion, step_completed_successfully,
};

pub(crate) struct PlanCompletionInput<'a> {
    pub(crate) workflow_name: &'a str,
    pub(crate) workflow_path: &'a Path,
    pub(crate) project_root: &'a Path,
    pub(crate) results: &'a [StepResult],
    pub(crate) completed_steps: &'a [usize],
    pub(crate) vars: &'a HashMap<String, String>,
    pub(crate) snapshot: &'a PlanCompletionSnapshot,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PlanCompletionSnapshot {
    initial_branch: Option<String>,
    initial_head: Option<String>,
}

impl PlanCompletionSnapshot {
    pub(crate) fn capture(workflow_name: &str, project_root: &Path) -> Self {
        dev2merge_completion::capture_snapshot(workflow_name, project_root)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlanSuccessReport {
    workflow_label: String,
    completion_summary: Option<String>,
}

impl PlanSuccessReport {
    pub(crate) fn new(workflow_label: &str, completion_summary: Option<String>) -> Self {
        Self {
            workflow_label: workflow_label.to_string(),
            completion_summary,
        }
    }

    fn render_summary_section(&self) -> String {
        let mut lines = vec![format!("Plan complete: {}", self.workflow_label)];
        if let Some(summary) = &self.completion_summary {
            lines.push(format!("Completion verification: {summary}"));
        }
        lines.join("\n")
    }

    fn render_details_section(&self) -> String {
        let mut details = String::new();
        details.push_str("# Plan Completion Report\n\n");
        details.push_str(&format!("- Workflow: `{}`\n", self.workflow_label));
        details.push_str("- Status: `success`\n");
        if let Some(summary) = &self.completion_summary {
            details.push_str(&format!("- Completion verification: {summary}\n"));
        }
        details
    }
}

pub(crate) fn persist_plan_success_output(
    session_dir: &Path,
    report: &PlanSuccessReport,
) -> Result<()> {
    if csa_session::read_section(session_dir, "summary")?.is_some() {
        return Ok(());
    }

    let summary = report.render_summary_section();
    let details = report.render_details_section();
    let marked = format!(
        "<!-- CSA:SECTION:summary -->\n{summary}\n<!-- CSA:SECTION:summary:END -->\n\n\
         <!-- CSA:SECTION:details -->\n{details}\n<!-- CSA:SECTION:details:END -->\n"
    );
    let marked = csa_session::redact_text_content(&marked);

    std::fs::create_dir_all(session_dir)
        .with_context(|| format!("Failed to create session dir: {}", session_dir.display()))?;
    csa_session::persist_structured_output(session_dir, &marked).with_context(|| {
        format!(
            "Failed to persist plan success output for {}",
            session_dir.display()
        )
    })?;
    Ok(())
}

pub(crate) fn persist_success_report_for_session(
    project_root: &Path,
    session_id: &str,
    report: &PlanSuccessReport,
) -> Result<()> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    persist_plan_success_output(&session_dir, report)
}

pub(crate) fn verify_plan_completion(
    input: PlanCompletionInput<'_>,
) -> std::result::Result<Option<String>, Box<PlanFailureError>> {
    if pr_bot_completion::is_pr_bot_workflow(input.workflow_name) {
        return pr_bot_completion::verify_pr_bot_completion(input);
    }
    if input.workflow_name == dev2merge_completion::DEV2MERGE_WORKFLOW_NAME {
        return dev2merge_completion::verify_plan_completion(input);
    }
    Ok(None)
}

#[cfg(test)]
#[path = "plan_cmd_completion_tests.rs"]
mod tests;
