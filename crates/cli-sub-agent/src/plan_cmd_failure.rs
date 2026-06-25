use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use csa_session::SessionArtifact;

use super::StepResult;

#[path = "plan_cmd_failure_detail.rs"]
mod failure_detail;

#[path = "plan_cmd_failure_recovery.rs"]
mod failure_recovery;
use failure_recovery::PlanFailureRecoveryReport;
pub(crate) use failure_recovery::capture_failure_recovery_snapshot;
#[cfg(test)]
use failure_recovery::{PlanFailureRecoverySnapshot, WEAVE_LOCK};

#[derive(Debug, Clone)]
pub(crate) struct FailedPlanStep {
    pub(crate) step_id: usize,
    pub(crate) title: String,
    pub(crate) exit_code: i32,
    pub(crate) skipped: bool,
    pub(crate) command: Option<String>,
    pub(crate) stderr_excerpt: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanFailureReport {
    workflow_name: String,
    workflow_path: String,
    summary: String,
    failed_steps: Vec<FailedPlanStep>,
    recovery: Option<PlanFailureRecoveryReport>,
}

impl PlanFailureReport {
    pub(crate) fn from_results(
        workflow_name: &str,
        workflow_path: &Path,
        summary: String,
        results: &[StepResult],
        recovery: Option<PlanFailureRecoveryReport>,
    ) -> Self {
        let failed_steps = results
            .iter()
            .filter(|result| result.exit_code != 0)
            .map(|result| FailedPlanStep {
                step_id: result.step_id,
                title: result.title.clone(),
                exit_code: result.exit_code,
                skipped: result.skipped,
                command: redact_optional_text(&result.command),
                stderr_excerpt: redact_optional_text(&result.stderr),
                error: redact_optional_text(&result.error),
            })
            .collect();

        Self {
            workflow_name: workflow_name.to_string(),
            workflow_path: workflow_path.display().to_string(),
            summary: csa_session::redact_text_content(&summary),
            failed_steps,
            recovery,
        }
    }

    pub(crate) fn summary_line(&self, workflow_label: &str) -> String {
        let failed_step = self.failed_steps.iter().find(|step| !step.skipped);
        let mut line = format!("plan failed: {workflow_label}: {}", self.summary);
        if let Some(step) = failed_step {
            line.push_str(&format!(
                "; failed_step={} exit_code={}",
                step.step_id, step.exit_code
            ));
            if let Some(detail) = step.actionable_failure_detail() {
                line.push_str(&format!(" detail={detail}"));
            }
        }
        if let Some(recovery) = &self.recovery
            && let Some(summary) = recovery.compact_summary_fragment()
        {
            line.push_str(&format!(" recovery={summary}"));
        }
        line
    }

    pub(crate) fn render_summary_section(&self) -> String {
        let mut lines = vec![
            format!("Plan failed: {}", self.workflow_name),
            format!("Summary: {}", self.summary),
        ];
        if let Some(step) = self.failed_steps.iter().find(|step| !step.skipped) {
            lines.push(format!(
                "Failed step: {} ({}) exited {}",
                step.step_id, step.title, step.exit_code
            ));
            if let Some(detail) = step.actionable_failure_detail() {
                lines.push(format!("Failure detail: {detail}"));
            }
            if let Some(hint) = step.recovery_hint() {
                lines.push(format!("Recovery hint: {hint}"));
            }
        }
        if let Some(recovery) = &self.recovery {
            lines.extend(recovery.render_summary_lines());
        }
        lines.join("\n")
    }

    pub(crate) fn render_details_section(&self) -> String {
        let mut details = String::new();
        details.push_str("# Plan Failure Report\n\n");
        details.push_str(&format!("- Workflow: `{}`\n", self.workflow_name));
        details.push_str(&format!("- Workflow path: `{}`\n", self.workflow_path));
        details.push_str(&format!("- Summary: {}\n", self.summary));

        details.push_str("\n## Failed Steps\n");
        if self.failed_steps.is_empty() {
            details.push_str("\nNo failed step records were captured.\n");
        }
        for step in &self.failed_steps {
            details.push_str(&format!("\n### Step {}: {}\n\n", step.step_id, step.title));
            details.push_str(&format!("- Exit code: `{}`\n", step.exit_code));
            details.push_str(&format!("- Skipped: `{}`\n", step.skipped));
            if let Some(hint) = step.recovery_hint() {
                details.push_str(&format!("- Recovery hint: {hint}\n"));
            }
            if let Some(command) = step.command.as_deref().filter(|value| !value.is_empty()) {
                details.push_str("\nCommand:\n\n```bash\n");
                details.push_str(command.trim_end());
                details.push_str("\n```\n");
            }
            if let Some(stderr) = step
                .stderr_excerpt
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                details.push_str("\nStderr excerpt:\n\n```text\n");
                details.push_str(stderr.trim_end());
                details.push_str("\n```\n");
            }
            if let Some(error) = step.error.as_deref().filter(|value| !value.is_empty()) {
                details.push_str("\nError:\n\n```text\n");
                details.push_str(error.trim_end());
                details.push_str("\n```\n");
            }
        }

        if let Some(recovery) = &self.recovery {
            details.push_str("\n## Recovery\n\n");
            recovery.render_markdown_into(&mut details);
        }

        details
    }
}

impl FailedPlanStep {
    fn actionable_failure_detail(&self) -> Option<String> {
        self.error
            .as_deref()
            .and_then(failure_detail::select_actionable_failure_line)
            .or_else(|| {
                self.stderr_excerpt
                    .as_deref()
                    .and_then(failure_detail::select_actionable_failure_line)
            })
    }

    fn recovery_hint(&self) -> Option<&'static str> {
        let command_mentions_mktd = self
            .command
            .as_deref()
            .is_some_and(|command| command.contains("patterns/mktd/workflow.toml"));
        if self.title.to_ascii_lowercase().contains("mktd") || command_mentions_mktd {
            return Some(
                "Inspect the mktd failure detail above, fix the reported mktd/TODO validation error, then rerun dev2merge. The parent structured output is the diagnostic source; hidden child transcripts should not be required.",
            );
        }
        None
    }
}

#[derive(Debug)]
pub(crate) struct PlanFailureError {
    summary: String,
    report: PlanFailureReport,
}

impl PlanFailureError {
    pub(crate) fn new(summary: String, report: PlanFailureReport) -> Self {
        Self { summary, report }
    }

    pub(crate) fn report(&self) -> &PlanFailureReport {
        &self.report
    }
}

impl fmt::Display for PlanFailureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary)
    }
}

impl std::error::Error for PlanFailureError {}

pub(crate) fn persist_plan_failure_output(
    session_dir: &Path,
    report: &PlanFailureReport,
) -> Result<()> {
    let summary = report.render_summary_section();
    let details = report.render_details_section();
    let marked = format!(
        "<!-- CSA:SECTION:summary -->\n{summary}\n<!-- CSA:SECTION:summary:END -->\n\n\
         <!-- CSA:SECTION:details -->\n{details}\n<!-- CSA:SECTION:details:END -->\n"
    );
    let marked = csa_session::redact_text_content(&marked);

    std::fs::create_dir_all(session_dir)
        .with_context(|| format!("Failed to create session dir: {}", session_dir.display()))?;
    std::fs::write(session_dir.join("output.log"), &marked).with_context(|| {
        format!(
            "Failed to write {}",
            session_dir.join("output.log").display()
        )
    })?;
    csa_session::persist_structured_output(session_dir, &marked).with_context(|| {
        format!(
            "Failed to persist plan failure output for {}",
            session_dir.display()
        )
    })?;
    Ok(())
}

pub(crate) fn report_from_error(error: Option<&anyhow::Error>) -> Option<PlanFailureReport> {
    let err = error?;
    err.downcast_ref::<PlanFailureError>()
        .map(|err| err.report().clone())
        .or_else(|| {
            err.downcast_ref::<Box<PlanFailureError>>()
                .map(|err| err.report().clone())
        })
}

pub(crate) fn persist_report_for_session(
    project_root: &Path,
    session_id: &str,
    report: &PlanFailureReport,
) -> Result<()> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    persist_plan_failure_output(&session_dir, report)
}

pub(crate) fn session_artifacts(
    workflow_label: &str,
    failure_report: Option<&PlanFailureReport>,
) -> Vec<SessionArtifact> {
    let mut artifacts = vec![SessionArtifact::new(workflow_label.to_string())];
    if failure_report.is_some() {
        artifacts.push(SessionArtifact::new("output/summary.md"));
        artifacts.push(SessionArtifact::new("output/details.md"));
    }
    artifacts
}

fn redact_optional_text(value: &Option<String>) -> Option<String> {
    value.as_deref().map(csa_session::redact_text_content)
}

#[cfg(test)]
#[path = "plan_cmd_failure_recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "plan_cmd_failure_tests.rs"]
mod tests;
