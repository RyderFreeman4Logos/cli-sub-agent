use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::StepResult;
use super::plan_cmd_failure::{PlanFailureError, PlanFailureReport};

#[path = "plan_cmd_pr_bot_completion.rs"]
mod pr_bot_completion;
pub(crate) use pr_bot_completion::{
    PrBotFailureSideEffectInput, verify_pr_bot_failure_side_effects,
};

const DEV2MERGE_WORKFLOW_NAME: &str = "dev2merge";
const DEV2MERGE_VERIFY_STEP_ID: usize = 18;
const DEV2MERGE_PUBLISH_STEPS: [usize; 5] = [13, 14, 15, 16, 17];

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
}

impl PlanCompletionSnapshot {
    pub(crate) fn capture(workflow_name: &str, project_root: &Path) -> Self {
        if workflow_name != DEV2MERGE_WORKFLOW_NAME {
            return Self::default();
        }
        Self {
            initial_branch: git_stdout(project_root, &["branch", "--show-current"])
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != "HEAD"),
        }
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
    if input.workflow_name != DEV2MERGE_WORKFLOW_NAME {
        return Ok(None);
    }
    let facts = Dev2MergeCompletionFacts::from_input(&input);
    if !facts.publish_started {
        return Ok(None);
    }

    let observed = observe_dev2merge_state(input.project_root, &facts);
    let failures = evaluate_dev2merge_completion(&facts, &observed);
    if failures.is_empty() {
        let pr = facts
            .pr_number
            .as_deref()
            .map(|value| format!("PR #{value} is MERGED"))
            .unwrap_or_else(|| "publish side effects are complete".to_string());
        let branch = facts
            .branch
            .as_deref()
            .map(|value| format!(" for branch {value}"))
            .unwrap_or_default();
        return Ok(Some(format!("dev2merge{branch}: {pr}")));
    }

    Err(Box::new(dev2merge_completion_failure_report(
        input.workflow_name,
        input.workflow_path,
        failures,
    )))
}

#[derive(Debug, Clone, Default)]
struct Dev2MergeCompletionFacts {
    publish_started: bool,
    push_gate_completed: bool,
    review_verdict_completed: bool,
    pr_completed: bool,
    pr_bot_completed: bool,
    post_merge_sync_completed: bool,
    branch: Option<String>,
    pr_number: Option<String>,
    pr_bot_done_marker: Option<PathBuf>,
}

impl Dev2MergeCompletionFacts {
    fn from_input(input: &PlanCompletionInput<'_>) -> Self {
        let dev2merge_skip = truthy(input.vars.get("DEV2MERGE_SKIP"));
        let step_completed = |step_id| {
            step_completed_successfully(
                input.results,
                input.completed_steps,
                dev2merge_skip,
                step_id,
            )
        };
        let publish_started = DEV2MERGE_PUBLISH_STEPS.into_iter().any(&step_completed);
        let branch = input
            .snapshot
            .initial_branch
            .clone()
            .or_else(|| non_empty_var(input.vars, "WORKFLOW_BRANCH"))
            .or_else(|| non_empty_var(input.vars, "BRANCH"));
        let pr_number = non_empty_var(input.vars, "PR_NUMBER")
            .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()));
        let pr_bot_done_marker = non_empty_var(input.vars, "PR_BOT_DONE_MARKER").map(PathBuf::from);

        Self {
            publish_started,
            push_gate_completed: step_completed(13),
            review_verdict_completed: step_completed(14),
            pr_completed: step_completed(15),
            pr_bot_completed: step_completed(16),
            post_merge_sync_completed: step_completed(17),
            branch,
            pr_number,
            pr_bot_done_marker,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Dev2MergeObservedState {
    pr_bot_marker_exists: Option<bool>,
    pr_state: Option<String>,
    pr_state_error: Option<String>,
}

fn observe_dev2merge_state(
    project_root: &Path,
    facts: &Dev2MergeCompletionFacts,
) -> Dev2MergeObservedState {
    let pr_bot_marker_exists = facts
        .pr_bot_done_marker
        .as_ref()
        .map(|path| resolve_observed_path(project_root, path).is_file());
    let (pr_state, pr_state_error) = match facts.pr_number.as_deref() {
        Some(pr_number) => match command_stdout(
            project_root,
            "gh",
            &["pr", "view", pr_number, "--json", "state", "-q", ".state"],
        ) {
            Ok(value) => (Some(value.trim().to_string()), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };

    Dev2MergeObservedState {
        pr_bot_marker_exists,
        pr_state,
        pr_state_error,
    }
}

fn evaluate_dev2merge_completion(
    facts: &Dev2MergeCompletionFacts,
    observed: &Dev2MergeObservedState,
) -> Vec<String> {
    let mut failures = Vec::new();
    if !facts.push_gate_completed {
        failures.push(
            "ERROR: Push Gate (Step 13) did not complete; branch publication is incomplete."
                .to_string(),
        );
    }
    if !facts.review_verdict_completed {
        failures.push("ERROR: Pre-PR Review Verdict Check (Step 14) did not complete.".to_string());
    }
    if !facts.pr_completed {
        failures
            .push("ERROR: Create or Reuse Pull Request (Step 15) did not complete.".to_string());
    }
    if !facts.pr_bot_completed {
        failures.push("ERROR: pr-bot Review & Merge Gate (Step 16) did not complete.".to_string());
    }
    if !facts.post_merge_sync_completed {
        failures.push("ERROR: Post-Merge Local Sync (Step 17) did not complete.".to_string());
    }

    match facts.pr_number.as_deref() {
        Some(_) => {}
        None => failures.push(
            "ERROR: PR_NUMBER was not captured; PR creation/reuse is incomplete.".to_string(),
        ),
    }
    match (&facts.pr_bot_done_marker, observed.pr_bot_marker_exists) {
        (Some(path), Some(true)) => {
            let _ = path;
        }
        (Some(path), Some(false)) => failures.push(format!(
            "ERROR: pr-bot completion marker is missing: {}.",
            path.display()
        )),
        (None, _) => failures.push(
            "ERROR: PR_BOT_DONE_MARKER was not captured; pr-bot completion is unverified."
                .to_string(),
        ),
        (Some(path), None) => failures.push(format!(
            "ERROR: pr-bot completion marker could not be inspected: {}.",
            path.display()
        )),
    }
    if let Some(error) = &observed.pr_state_error {
        failures.push(format!(
            "ERROR: failed to inspect PR state via gh: {error}."
        ));
    } else if facts.pr_number.is_some() {
        match observed.pr_state.as_deref() {
            Some("MERGED") => {}
            Some("") => {
                failures.push("ERROR: PR state is empty; expected MERGED after pr-bot.".to_string())
            }
            Some(state) => failures.push(format!(
                "ERROR: PR state is {state}; expected MERGED after pr-bot."
            )),
            None => failures.push(
                "ERROR: PR state was not observed; expected MERGED after pr-bot.".to_string(),
            ),
        }
    }

    failures
}

fn dev2merge_completion_failure_report(
    workflow_name: &str,
    workflow_path: &Path,
    failures: Vec<String>,
) -> PlanFailureError {
    let summary = "dev2merge publish side-effect verification failed".to_string();
    let mut error = vec![
        "ERROR: dev2merge publish side-effect verification failed after workflow steps reported success."
            .to_string(),
    ];
    error.extend(failures);
    error.push(
        "Retry after resolving the named incomplete side effect; callers should use this structured result instead of hidden session transcripts."
            .to_string(),
    );
    let error = error.join("\n");
    let synthetic_result = StepResult {
        step_id: DEV2MERGE_VERIFY_STEP_ID,
        title: "Dev2merge Publish Side-Effect Verification".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(error.clone()),
        output: None,
        session_id: None,
        command: Some(
            "post-run verifier: check Step 13-17 completion, PR_NUMBER, PR_BOT_DONE_MARKER, and gh PR state"
                .to_string(),
        ),
        stderr: Some(error),
    };
    let report = PlanFailureReport::from_results(
        workflow_name,
        workflow_path,
        summary.clone(),
        &[synthetic_result],
        None,
    );
    PlanFailureError::new(summary, report)
}

fn step_completed_successfully(
    results: &[StepResult],
    completed_steps: &[usize],
    dev2merge_skip: bool,
    step_id: usize,
) -> bool {
    if let Some(result) = results.iter().find(|result| result.step_id == step_id) {
        return !result.skipped && result.exit_code == 0;
    }
    !dev2merge_skip && completed_steps.contains(&step_id)
}

fn truthy(value: Option<&String>) -> bool {
    value
        .map(|value| {
            let lower = value.trim().to_ascii_lowercase();
            !lower.is_empty() && lower != "false" && lower != "0"
        })
        .unwrap_or(false)
}

fn non_empty_var(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_observed_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn git_stdout(project_root: &Path, args: &[&str]) -> Result<String> {
    command_stdout(project_root, "git", args).map_err(anyhow::Error::msg)
}

fn command_stdout(project_root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|error| format!("{program} {} failed to start: {error}", args.join(" ")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!(
        "{program} {} exited {}{}",
        args.join(" "),
        output.status.code().unwrap_or(1),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

#[cfg(test)]
#[path = "plan_cmd_completion_tests.rs"]
mod tests;
