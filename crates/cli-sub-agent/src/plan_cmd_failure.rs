use std::fmt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use csa_session::SessionArtifact;

use super::StepResult;

#[path = "plan_cmd_failure_detail.rs"]
mod failure_detail;

const PR_BOT_WORKFLOW_NAME: &str = "pr-bot";
const WEAVE_LOCK: &str = "weave.lock";
const CSA_PLAN_STATE_PREFIX: &str = ".csa/state/plan/";

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
        }
        if let Some(recovery) = &self.recovery {
            lines.push(format!("Recovery status: {}", recovery.status.as_str()));
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
        self.stderr_excerpt
            .as_deref()
            .and_then(failure_detail::select_actionable_failure_line)
            .or_else(|| {
                self.error
                    .as_deref()
                    .and_then(failure_detail::select_actionable_failure_line)
            })
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
    error
        .and_then(|err| err.downcast_ref::<PlanFailureError>())
        .map(|err| err.report().clone())
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

pub(crate) fn capture_failure_recovery_snapshot(
    workflow_name: &str,
    project_root: &Path,
) -> Option<PlanFailureRecoverySnapshot> {
    (workflow_name == PR_BOT_WORKFLOW_NAME)
        .then(|| PlanFailureRecoverySnapshot::capture(project_root))
}

#[derive(Debug, Clone)]
pub(crate) struct PlanFailureRecoverySnapshot {
    initial_ref: Option<CheckoutRef>,
    initial_status: Option<Vec<String>>,
    capture_error: Option<String>,
}

impl PlanFailureRecoverySnapshot {
    fn capture(project_root: &Path) -> Self {
        if let Err(error) = run_git(project_root, &["rev-parse", "--is-inside-work-tree"]) {
            return Self {
                initial_ref: None,
                initial_status: None,
                capture_error: Some(format!("not a git worktree: {error}")),
            };
        }

        let initial_ref = match current_checkout_ref(project_root) {
            Ok(value) => Some(value),
            Err(error) => {
                return Self {
                    initial_ref: None,
                    initial_status: None,
                    capture_error: Some(format!("failed to capture initial checkout: {error}")),
                };
            }
        };
        let initial_status = match git_recovery_status_lines(project_root) {
            Ok(value) => Some(value),
            Err(error) => {
                return Self {
                    initial_ref,
                    initial_status: None,
                    capture_error: Some(format!("failed to capture initial git status: {error}")),
                };
            }
        };

        Self {
            initial_ref,
            initial_status,
            capture_error: None,
        }
    }

    pub(crate) fn recover_after_failure(&self, project_root: &Path) -> PlanFailureRecoveryReport {
        let initial_ref_label = self.initial_ref.as_ref().map(CheckoutRef::label);
        let current_ref_before = current_checkout_ref(project_root)
            .ok()
            .map(|value| value.label());

        if let Some(error) = &self.capture_error {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label,
                current_ref_before,
                vec![format!(
                    "Automatic recovery skipped because the initial checkout snapshot failed: {error}."
                )],
                vec!["git status --short --branch".to_string()],
            );
        }

        let Some(initial_status) = &self.initial_status else {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label,
                current_ref_before,
                vec![
                    "Automatic recovery skipped because the initial git status is unavailable."
                        .to_string(),
                ],
                vec!["git status --short --branch".to_string()],
            );
        };
        if !initial_status.is_empty() {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label.clone(),
                current_ref_before,
                vec![
                    "Automatic recovery skipped because the worktree was already dirty before pr-bot started.".to_string(),
                    "Resolve the pre-existing changes before retrying pr-bot.".to_string(),
                ],
                recovery_commands(initial_ref_label.as_deref()),
            );
        }

        let mut actions = Vec::new();
        let status_before_cleanup = match git_recovery_status_lines(project_root) {
            Ok(lines) => lines,
            Err(error) => {
                return PlanFailureRecoveryReport::manual(
                    initial_ref_label.clone(),
                    current_ref_before,
                    vec![format!(
                        "Could not inspect failed worktree status: {error}."
                    )],
                    recovery_commands(initial_ref_label.as_deref()),
                );
            }
        };

        let weave_lock_changed_after_snapshot = only_weave_lock_dirty(&status_before_cleanup);
        if weave_lock_changed_after_snapshot {
            actions.push(
                "Preserved dirty weave.lock because automatic recovery cannot prove it was \
                 created by this pr-bot run."
                    .to_string(),
            );
        } else if !status_before_cleanup.is_empty() {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label.clone(),
                current_ref_before,
                vec![
                    "Automatic recovery will not modify dirty paths that it cannot prove were created by pr-bot.".to_string(),
                    format!(
                        "Current dirty paths require manual review: {}",
                        status_before_cleanup.join("; ")
                    ),
                ],
                recovery_commands(initial_ref_label.as_deref()),
            );
        }

        if let Some(initial_ref) = &self.initial_ref {
            match current_checkout_ref(project_root) {
                Ok(current_ref) if &current_ref == initial_ref => {}
                Ok(_) | Err(_) => match restore_checkout(project_root, initial_ref) {
                    Ok(()) => {
                        actions.push(format!("Restored checkout to {}.", initial_ref.label()))
                    }
                    Err(error) => {
                        let mut messages = actions;
                        messages.push(format!(
                            "Could not restore checkout to {} automatically: {error}.",
                            initial_ref.label()
                        ));
                        return PlanFailureRecoveryReport::manual(
                            initial_ref_label.clone(),
                            current_ref_before,
                            messages,
                            recovery_commands(initial_ref_label.as_deref()),
                        );
                    }
                },
            }
        }

        let final_status = git_recovery_status_lines(project_root).unwrap_or_else(|error| {
            vec![format!(
                "failed to inspect final status after recovery: {error}"
            )]
        });
        let current_ref_after = current_checkout_ref(project_root)
            .ok()
            .map(|value| value.label());
        let checkout_restored = match &self.initial_ref {
            Some(initial_ref) => {
                current_checkout_ref(project_root).is_ok_and(|current| &current == initial_ref)
            }
            None => true,
        };
        if final_status.is_empty() && checkout_restored {
            let status = if actions.is_empty() {
                PlanRecoveryStatus::NotNeeded
            } else {
                PlanRecoveryStatus::Restored
            };
            return PlanFailureRecoveryReport {
                status,
                initial_ref: initial_ref_label,
                current_ref_before,
                current_ref_after,
                messages: if actions.is_empty() {
                    vec!["No checkout or weave.lock cleanup was needed.".to_string()]
                } else {
                    actions
                },
                recovery_commands: Vec::new(),
                final_status,
            };
        }
        if weave_lock_changed_after_snapshot
            && checkout_restored
            && only_weave_lock_dirty(&final_status)
        {
            let mut messages = actions;
            messages.push("Review weave.lock before retrying pr-bot.".to_string());
            return PlanFailureRecoveryReport::manual_with_refs(
                initial_ref_label,
                current_ref_before,
                current_ref_after,
                messages,
                vec!["git status --short --branch".to_string()],
                final_status,
            );
        }

        PlanFailureRecoveryReport::manual_with_refs(
            initial_ref_label,
            current_ref_before,
            current_ref_after,
            vec![
                "Automatic recovery ran but the checkout or worktree is still not restored."
                    .to_string(),
            ],
            vec!["git status --short --branch".to_string()],
            final_status,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckoutRef {
    Branch(String),
    Detached(String),
}

impl CheckoutRef {
    fn label(&self) -> String {
        match self {
            Self::Branch(branch) => branch.clone(),
            Self::Detached(head) => head.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlanFailureRecoveryReport {
    status: PlanRecoveryStatus,
    initial_ref: Option<String>,
    current_ref_before: Option<String>,
    current_ref_after: Option<String>,
    messages: Vec<String>,
    recovery_commands: Vec<String>,
    final_status: Vec<String>,
}

impl PlanFailureRecoveryReport {
    fn manual(
        initial_ref: Option<String>,
        current_ref: Option<String>,
        messages: Vec<String>,
        recovery_commands: Vec<String>,
    ) -> Self {
        Self::manual_with_refs(
            initial_ref,
            current_ref.clone(),
            current_ref,
            messages,
            recovery_commands,
            Vec::new(),
        )
    }

    fn manual_with_refs(
        initial_ref: Option<String>,
        current_ref_before: Option<String>,
        current_ref_after: Option<String>,
        messages: Vec<String>,
        recovery_commands: Vec<String>,
        final_status: Vec<String>,
    ) -> Self {
        Self {
            status: PlanRecoveryStatus::ManualRequired,
            initial_ref,
            current_ref_before,
            current_ref_after,
            messages,
            recovery_commands,
            final_status,
        }
    }

    fn render_markdown_into(&self, output: &mut String) {
        output.push_str(&format!("- Recovery status: `{}`\n", self.status.as_str()));
        if let Some(initial_ref) = &self.initial_ref {
            output.push_str(&format!("- Initial checkout: `{initial_ref}`\n"));
        }
        if let Some(current_ref) = &self.current_ref_before {
            output.push_str(&format!("- Checkout at failure: `{current_ref}`\n"));
        }
        if let Some(current_ref) = &self.current_ref_after {
            output.push_str(&format!("- Checkout after recovery: `{current_ref}`\n"));
        }
        if !self.messages.is_empty() {
            output.push_str("\nMessages:\n");
            for message in &self.messages {
                output.push_str(&format!("- {message}\n"));
            }
        }
        if !self.final_status.is_empty() {
            output.push_str("\nRemaining git status:\n\n```text\n");
            output.push_str(&self.final_status.join("\n"));
            output.push_str("\n```\n");
        }
        if !self.recovery_commands.is_empty() {
            output.push_str("\nManual recovery commands:\n\n```bash\n");
            for command in &self.recovery_commands {
                output.push_str(command);
                output.push('\n');
            }
            output.push_str("```\n");
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PlanRecoveryStatus {
    Restored,
    NotNeeded,
    ManualRequired,
}

impl PlanRecoveryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::NotNeeded => "not-needed",
            Self::ManualRequired => "manual-required",
        }
    }
}

fn current_checkout_ref(project_root: &Path) -> Result<CheckoutRef> {
    match run_git(
        project_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    ) {
        Ok(branch) if !branch.trim().is_empty() => {
            Ok(CheckoutRef::Branch(branch.trim().to_string()))
        }
        Ok(_) | Err(_) => {
            let head = run_git(project_root, &["rev-parse", "--verify", "HEAD"])?;
            Ok(CheckoutRef::Detached(head.trim().to_string()))
        }
    }
}

fn git_status_lines(project_root: &Path) -> Result<Vec<String>> {
    let status = run_git(
        project_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    Ok(status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_recovery_status_lines(project_root: &Path) -> Result<Vec<String>> {
    Ok(git_status_lines(project_root)?
        .into_iter()
        .filter(|line| !is_csa_plan_state_status(line))
        .collect())
}

fn is_csa_plan_state_status(line: &str) -> bool {
    porcelain_path(line).is_some_and(|path| path.starts_with(CSA_PLAN_STATE_PREFIX))
}

fn only_weave_lock_dirty(status_lines: &[String]) -> bool {
    !status_lines.is_empty()
        && status_lines
            .iter()
            .all(|line| porcelain_path(line).is_some_and(|path| path == WEAVE_LOCK))
}

fn porcelain_path(line: &str) -> Option<&str> {
    line.get(3..).map(str::trim).map(|path| {
        path.strip_prefix('"')
            .and_then(|path| path.strip_suffix('"'))
            .unwrap_or(path)
    })
}

fn restore_checkout(project_root: &Path, initial_ref: &CheckoutRef) -> Result<()> {
    match initial_ref {
        CheckoutRef::Branch(branch) => run_git_status(project_root, &["switch", branch]),
        CheckoutRef::Detached(head) => {
            run_git_status(project_root, &["checkout", "--detach", head])
        }
    }
}

fn redact_optional_text(value: &Option<String>) -> Option<String> {
    value.as_deref().map(csa_session::redact_text_content)
}

fn recovery_commands(initial_ref: Option<&str>) -> Vec<String> {
    let mut commands = Vec::new();
    if let Some(initial_ref) = initial_ref {
        commands.push(format!(
            "git switch {}",
            super::shell_escape_for_command(initial_ref)
        ));
    }
    commands.push("git status --short --branch".to_string());
    commands
}

fn run_git(project_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed with status {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn run_git_status(project_root: &Path, args: &[&str]) -> Result<()> {
    run_git(project_root, args).map(|_| ())
}

#[cfg(test)]
#[path = "plan_cmd_failure_tests.rs"]
mod tests;
