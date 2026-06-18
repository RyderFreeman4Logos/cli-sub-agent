use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::plan_cmd::StepResult;
use crate::plan_cmd::plan_cmd_completion::PlanCompletionInput;
use crate::plan_cmd::plan_cmd_failure::{PlanFailureError, PlanFailureReport};

const PR_BOT_WORKFLOW_NAME: &str = "pr-bot";
const PR_BOT_VERIFY_STEP_ID: usize = 26;
const PR_BOT_MERGE_STEP_IDS: [usize; 2] = [22, 23];
const PR_BOT_POST_MERGE_STEP_ID: usize = 24;

pub(crate) struct PrBotFailureSideEffectInput<'a> {
    pub(crate) workflow_name: &'a str,
    pub(crate) workflow_path: &'a Path,
    pub(crate) project_root: &'a Path,
    pub(crate) results: &'a [StepResult],
    pub(crate) completed_steps: &'a [usize],
    pub(crate) vars: &'a HashMap<String, String>,
    pub(crate) failure_summary: &'a str,
}

#[derive(Debug, Clone, Default)]
struct PrBotMergeFacts {
    pr_number: Option<String>,
    merge_verify_ref: Option<String>,
    repo: Option<String>,
    merge_completed_marker: bool,
    merge_step_completed: bool,
    post_merge_step_completed: bool,
    merge_step_attempted: bool,
}

#[derive(Debug, Clone, Default)]
struct PrBotObservedState {
    pr_state: Option<String>,
    pr_state_error: Option<String>,
}

pub(crate) fn is_pr_bot_workflow(workflow_name: &str) -> bool {
    workflow_name == PR_BOT_WORKFLOW_NAME
}

pub(crate) fn verify_pr_bot_completion(
    input: PlanCompletionInput<'_>,
) -> std::result::Result<Option<String>, Box<PlanFailureError>> {
    let facts = PrBotMergeFacts::from_completion_input(&input);
    if !facts.should_verify_success() {
        return Ok(None);
    }

    let observed = observe_pr_bot_state(input.project_root, &facts);
    match evaluate_pr_bot_success(&facts, &observed) {
        Ok(Some(summary)) => Ok(Some(summary)),
        Ok(None) => Ok(None),
        Err(failures) => Err(Box::new(pr_bot_completion_failure_report(
            input.workflow_name,
            input.workflow_path,
            failures,
        ))),
    }
}

pub(crate) fn verify_pr_bot_failure_side_effects(
    input: PrBotFailureSideEffectInput<'_>,
) -> Option<PlanFailureError> {
    if !is_pr_bot_workflow(input.workflow_name) {
        return None;
    }

    let facts = PrBotMergeFacts::from_failure_input(&input);
    facts.pr_reference()?;

    let observed = observe_pr_bot_state(input.project_root, &facts);
    verify_pr_bot_failure_side_effects_with_observed(&input, &facts, &observed)
}

fn verify_pr_bot_failure_side_effects_with_observed(
    input: &PrBotFailureSideEffectInput<'_>,
    facts: &PrBotMergeFacts,
    observed: &PrBotObservedState,
) -> Option<PlanFailureError> {
    if observed.pr_state.as_deref() != Some("MERGED") {
        return None;
    }

    Some(pr_bot_merged_despite_failure_report(
        input.workflow_name,
        input.workflow_path,
        input.failure_summary,
        input.results,
        facts,
        observed,
    ))
}

impl PrBotMergeFacts {
    fn from_completion_input(input: &PlanCompletionInput<'_>) -> Self {
        let merge_verify_ref = non_empty_var(input.vars, "MERGED_PR_VERIFY_REF")
            .or_else(|| output_assignment(input.results, "MERGED_PR_VERIFY_REF"))
            .or_else(|| non_empty_var(input.vars, "PR_NUM"));
        let pr_number = non_empty_var(input.vars, "PR_NUM")
            .or_else(|| output_assignment(input.results, "PR_NUM"));
        let repo = non_empty_var(input.vars, "REPO")
            .or_else(|| non_empty_var(input.vars, "REPO_SLUG"))
            .or_else(|| output_assignment(input.results, "REPO"))
            .or_else(|| output_assignment(input.results, "REPO_SLUG"));
        let merge_step_completed = PR_BOT_MERGE_STEP_IDS.into_iter().any(|step_id| {
            step_completed_successfully(input.results, input.completed_steps, step_id)
        });
        let post_merge_step_completed = step_completed_successfully(
            input.results,
            input.completed_steps,
            PR_BOT_POST_MERGE_STEP_ID,
        );
        let merge_step_attempted = PR_BOT_MERGE_STEP_IDS
            .into_iter()
            .any(|step_id| step_attempted(input.results, step_id));

        Self {
            pr_number,
            merge_verify_ref,
            repo,
            merge_completed_marker: truthy(input.vars.get("MERGE_COMPLETED"))
                || truthy(output_assignment(input.results, "MERGE_COMPLETED").as_ref()),
            merge_step_completed,
            post_merge_step_completed,
            merge_step_attempted,
        }
    }

    fn from_failure_input(input: &PrBotFailureSideEffectInput<'_>) -> Self {
        let merge_verify_ref = non_empty_var(input.vars, "MERGED_PR_VERIFY_REF")
            .or_else(|| output_assignment(input.results, "MERGED_PR_VERIFY_REF"))
            .or_else(|| non_empty_var(input.vars, "PR_NUM"));
        let pr_number = non_empty_var(input.vars, "PR_NUM")
            .or_else(|| output_assignment(input.results, "PR_NUM"));
        let repo = non_empty_var(input.vars, "REPO")
            .or_else(|| non_empty_var(input.vars, "REPO_SLUG"))
            .or_else(|| output_assignment(input.results, "REPO"))
            .or_else(|| output_assignment(input.results, "REPO_SLUG"));
        let merge_step_completed = PR_BOT_MERGE_STEP_IDS.into_iter().any(|step_id| {
            step_completed_successfully(input.results, input.completed_steps, step_id)
        });
        let post_merge_step_completed = step_completed_successfully(
            input.results,
            input.completed_steps,
            PR_BOT_POST_MERGE_STEP_ID,
        );
        let merge_step_attempted = PR_BOT_MERGE_STEP_IDS
            .into_iter()
            .any(|step_id| step_attempted(input.results, step_id));

        Self {
            pr_number,
            merge_verify_ref,
            repo,
            merge_completed_marker: truthy(input.vars.get("MERGE_COMPLETED"))
                || truthy(output_assignment(input.results, "MERGE_COMPLETED").as_ref()),
            merge_step_completed,
            post_merge_step_completed,
            merge_step_attempted,
        }
    }

    fn pr_reference(&self) -> Option<&str> {
        self.merge_verify_ref
            .as_deref()
            .or(self.pr_number.as_deref())
            .filter(|value| !value.trim().is_empty())
    }

    fn should_verify_success(&self) -> bool {
        self.pr_reference().is_some()
            && (self.merge_completed_marker
                || self.merge_step_completed
                || self.post_merge_step_completed
                || self.merge_step_attempted)
    }

    fn merge_evidence(&self) -> String {
        let mut evidence = Vec::new();
        if self.merge_completed_marker {
            evidence.push("MERGE_COMPLETED=true");
        }
        if self.merge_step_completed {
            evidence.push("final merge step completed");
        }
        if self.post_merge_step_completed {
            evidence.push("post-merge verification step completed");
        }
        if self.merge_step_attempted && !self.merge_step_completed {
            evidence.push("final merge step attempted before failure");
        }
        if evidence.is_empty() {
            "no local merge marker captured; observed state comes from GitHub".to_string()
        } else {
            evidence.join(", ")
        }
    }
}

fn evaluate_pr_bot_success(
    facts: &PrBotMergeFacts,
    observed: &PrBotObservedState,
) -> std::result::Result<Option<String>, Vec<String>> {
    let Some(pr_ref) = facts.pr_reference() else {
        return Ok(None);
    };

    if let Some(error) = &observed.pr_state_error {
        return Err(vec![format!(
            "ERROR: failed to inspect pr-bot PR state via gh for PR {pr_ref}: {error}."
        )]);
    }

    match observed.pr_state.as_deref() {
        Some("MERGED") => Ok(Some(format!("pr-bot: PR #{pr_ref} is MERGED"))),
        Some("") => Err(vec![format!(
            "ERROR: pr-bot PR {pr_ref} state is empty; expected MERGED."
        )]),
        Some(state) => Err(vec![format!(
            "ERROR: pr-bot PR {pr_ref} state is {state}; expected MERGED."
        )]),
        None => Err(vec![format!(
            "ERROR: pr-bot PR {pr_ref} state was not observed; expected MERGED."
        )]),
    }
}

fn observe_pr_bot_state(project_root: &Path, facts: &PrBotMergeFacts) -> PrBotObservedState {
    let Some(pr_ref) = facts.pr_reference() else {
        return PrBotObservedState::default();
    };

    match gh_pr_state(project_root, pr_ref, facts.repo.as_deref()) {
        Ok(value) => PrBotObservedState {
            pr_state: Some(value.trim().to_string()),
            pr_state_error: None,
        },
        Err(error) => PrBotObservedState {
            pr_state: None,
            pr_state_error: Some(error),
        },
    }
}

fn pr_bot_completion_failure_report(
    workflow_name: &str,
    workflow_path: &Path,
    failures: Vec<String>,
) -> PlanFailureError {
    let summary = "pr-bot merge side-effect verification failed".to_string();
    let mut error =
        vec!["ERROR: pr-bot completed but merge side-effect verification failed.".to_string()];
    error.extend(failures);
    error.push(
        "Use this structured result instead of hidden session transcripts; inspect the named PR and rerun pr-bot only if it is still unmerged."
            .to_string(),
    );
    let error = error.join("\n");
    let synthetic_result = StepResult {
        step_id: PR_BOT_VERIFY_STEP_ID,
        title: "pr-bot Merge Side-Effect Verification".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(error.clone()),
        output: None,
        session_id: None,
        command: Some(
            "post-run verifier: check MERGED_PR_VERIFY_REF/PR_NUM and gh PR state".to_string(),
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

fn pr_bot_merged_despite_failure_report(
    workflow_name: &str,
    workflow_path: &Path,
    failure_summary: &str,
    results: &[StepResult],
    facts: &PrBotMergeFacts,
    observed: &PrBotObservedState,
) -> PlanFailureError {
    let pr_ref = facts.pr_reference().unwrap_or("unknown");
    let summary = format!(
        "pr-bot observed PR #{pr_ref} is MERGED despite workflow failure; merge result takes precedence over failed decision summary"
    );
    let mut error = vec![
        format!(
            "ERROR: pr-bot failure summary contradicted the observed GitHub side effect for PR #{pr_ref}."
        ),
        format!(
            "Observed PR state: {}.",
            observed.pr_state.as_deref().unwrap_or("UNKNOWN")
        ),
        format!("Merge evidence: {}.", facts.merge_evidence()),
        format!("Original failure summary: {failure_summary}."),
    ];
    if let Some(failed_step) = first_failed_step(results) {
        error.push(format!(
            "Original failed step: {} ({}) exited {}.",
            failed_step.step_id, failed_step.title, failed_step.exit_code
        ));
        if let Some(detail) = failed_step_detail(failed_step) {
            error.push(format!("Original failure detail: {detail}"));
        }
    }
    error.push(
        "Treat this as a fail-closed audit contradiction: do not claim the PR is unmerged; inspect the merged PR, the failed step, and whether the blocking finding was posted before or after the merge."
            .to_string(),
    );
    let error = error.join("\n");
    let synthetic_result = StepResult {
        step_id: PR_BOT_VERIFY_STEP_ID,
        title: "pr-bot Merge Side-Effect Verification".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(error.clone()),
        output: None,
        session_id: None,
        command: Some(format!(
            "post-failure verifier: gh pr view {pr_ref} --json state -q .state"
        )),
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

fn first_failed_step(results: &[StepResult]) -> Option<&StepResult> {
    results
        .iter()
        .find(|result| !result.skipped && result.exit_code != 0)
}

fn failed_step_detail(step: &StepResult) -> Option<String> {
    step.error
        .as_deref()
        .or(step.stderr.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.lines().next().unwrap_or(value).to_string())
}

fn step_completed_successfully(
    results: &[StepResult],
    completed_steps: &[usize],
    step_id: usize,
) -> bool {
    if let Some(result) = results.iter().find(|result| result.step_id == step_id) {
        return !result.skipped && result.exit_code == 0;
    }
    completed_steps.contains(&step_id)
}

fn step_attempted(results: &[StepResult], step_id: usize) -> bool {
    results
        .iter()
        .any(|result| result.step_id == step_id && !result.skipped)
}

fn non_empty_var(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn output_assignment(results: &[StepResult], key: &str) -> Option<String> {
    results
        .iter()
        .rev()
        .filter_map(|result| result.output.as_deref())
        .flat_map(str::lines)
        .filter_map(|line| parse_assignment_line(line, key))
        .next()
}

fn parse_assignment_line(line: &str, key: &str) -> Option<String> {
    let payload = line.trim().strip_prefix("CSA_VAR:")?.trim();
    let (raw_key, raw_value) = payload.split_once('=')?;
    (raw_key.trim() == key)
        .then(|| raw_value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn truthy(value: Option<&String>) -> bool {
    value
        .map(|value| {
            let lower = value.trim().to_ascii_lowercase();
            !lower.is_empty() && lower != "false" && lower != "0"
        })
        .unwrap_or(false)
}

fn gh_pr_state(project_root: &Path, pr_ref: &str, repo: Option<&str>) -> Result<String, String> {
    let mut args = vec![
        "pr".to_string(),
        "view".to_string(),
        pr_ref.to_string(),
        "--json".to_string(),
        "state".to_string(),
        "-q".to_string(),
        ".state".to_string(),
    ];
    if let Some(repo) = repo.filter(|value| !value.trim().is_empty()) {
        args.push("--repo".to_string());
        args.push(repo.to_string());
    }
    command_stdout(project_root, "gh", &args)
}

fn command_stdout(project_root: &Path, program: &str, args: &[String]) -> Result<String, String> {
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
#[path = "plan_cmd_pr_bot_completion_tests.rs"]
mod tests;
