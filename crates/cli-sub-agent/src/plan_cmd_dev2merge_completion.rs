use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use super::{PlanCompletionInput, PlanCompletionSnapshot};
use crate::plan_cmd::StepResult;
use crate::plan_cmd::plan_cmd_failure::{PlanFailureError, PlanFailureReport};

pub(super) const DEV2MERGE_WORKFLOW_NAME: &str = "dev2merge";
const DEV2MERGE_VERIFY_STEP_ID: usize = 18;
const DEV2MERGE_ALREADY_RESOLVED_STEP_ID: usize = 0;
const DEV2MERGE_PUBLISH_STEPS: [usize; 5] = [13, 14, 15, 16, 17];

pub(super) fn capture_snapshot(workflow_name: &str, project_root: &Path) -> PlanCompletionSnapshot {
    if workflow_name != DEV2MERGE_WORKFLOW_NAME {
        return PlanCompletionSnapshot::default();
    }
    PlanCompletionSnapshot {
        initial_branch: git_trimmed(project_root, &["branch", "--show-current"])
            .filter(|value| value != "HEAD"),
        initial_head: git_trimmed(project_root, &["rev-parse", "--verify", "HEAD"]),
    }
}

pub(super) fn verify_plan_completion(
    input: PlanCompletionInput<'_>,
) -> std::result::Result<Option<String>, Box<PlanFailureError>> {
    let facts = Dev2MergeCompletionFacts::from_input(&input);
    if !facts.publish_started {
        if facts.already_resolved_skip {
            return Ok(Some(dev2merge_already_resolved_summary(&facts)));
        }
        return Err(Box::new(dev2merge_lifecycle_not_started_failure_report(
            input.workflow_name,
            input.workflow_path,
            &facts,
        )));
    }

    let observed = observe_dev2merge_state(input.project_root, &facts);
    let failures = evaluate_dev2merge_completion(&facts, &observed);
    if failures.is_empty() {
        return Ok(Some(dev2merge_success_summary(&facts, &observed)));
    }

    Err(Box::new(dev2merge_completion_failure_report(
        input.workflow_name,
        input.workflow_path,
        failures,
    )))
}

#[derive(Debug, Clone, Default)]
pub(super) struct Dev2MergeCompletionFacts {
    pub(super) dev2merge_skip: bool,
    pub(super) already_resolved_skip: bool,
    pub(super) publish_started: bool,
    pub(super) push_gate_completed: bool,
    pub(super) review_verdict_completed: bool,
    pub(super) pr_completed: bool,
    pub(super) pr_bot_completed: bool,
    pub(super) post_merge_sync_completed: bool,
    pub(super) branch: Option<String>,
    pub(super) default_branch: Option<String>,
    pub(super) issue_number: Option<String>,
    pub(super) pr_number: Option<String>,
    pub(super) pr_bot_done_marker: Option<PathBuf>,
    pub(super) review_completed: bool,
    pub(super) review_verdict_checked: bool,
    pub(super) pushed: bool,
    pub(super) already_resolved_message: Option<String>,
    pub(super) initial_head: Option<String>,
}

impl Dev2MergeCompletionFacts {
    fn from_input(input: &PlanCompletionInput<'_>) -> Self {
        let dev2merge_skip = truthy(input.vars.get("DEV2MERGE_SKIP"));
        let already_resolved_skip =
            dev2merge_skip && already_resolved_step_declared_skip(input.results);
        let step_completed = |step_id| {
            step_completed_successfully(
                input.results,
                input.completed_steps,
                dev2merge_skip,
                step_id,
            )
        };
        let branch = input
            .snapshot
            .initial_branch
            .clone()
            .or_else(|| non_empty_var(input.vars, "WORKFLOW_BRANCH"))
            .or_else(|| non_empty_var(input.vars, "BRANCH"));
        Self {
            dev2merge_skip,
            already_resolved_skip,
            publish_started: DEV2MERGE_PUBLISH_STEPS.into_iter().any(&step_completed),
            push_gate_completed: step_completed(13),
            review_verdict_completed: step_completed(14),
            pr_completed: step_completed(15),
            pr_bot_completed: step_completed(16),
            post_merge_sync_completed: step_completed(17),
            branch,
            default_branch: non_empty_var(input.vars, "DEFAULT_BRANCH"),
            issue_number: numeric_var(input.vars, "ISSUE_NUMBER"),
            pr_number: numeric_var(input.vars, "PR_NUMBER"),
            pr_bot_done_marker: non_empty_var(input.vars, "PR_BOT_DONE_MARKER").map(PathBuf::from),
            review_completed: truthy(input.vars.get("REVIEW_COMPLETED")),
            review_verdict_checked: truthy(input.vars.get("REVIEW_VERDICT_CHECKED")),
            pushed: truthy(input.vars.get("PUSHED")),
            already_resolved_message: already_resolved_skip_message(input.results),
            initial_head: input.snapshot.initial_head.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Dev2MergeObservedState {
    pub(super) pr_bot_marker_exists: Option<bool>,
    pub(super) pr_state: Option<String>,
    pub(super) pr_state_error: Option<String>,
    pub(super) current_branch: Option<String>,
    pub(super) current_head: Option<String>,
    pub(super) branch_head: Option<String>,
    pub(super) branch_moved: Option<bool>,
    pub(super) implementation_commits_ahead: Option<usize>,
    pub(super) implementation_commits_created: Option<usize>,
    pub(super) implementation_diff_paths: Option<usize>,
    pub(super) implementation_diff_empty: Option<bool>,
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
    let current_branch =
        git_trimmed(project_root, &["branch", "--show-current"]).filter(|value| value != "HEAD");
    let current_head = git_trimmed(project_root, &["rev-parse", "--verify", "HEAD"]);
    let branch_head = facts
        .branch
        .as_deref()
        .and_then(|branch| git_trimmed(project_root, &["rev-parse", "--verify", branch]));
    let branch_moved = facts
        .initial_head
        .as_deref()
        .zip(branch_head.as_deref())
        .map(|(initial, branch)| initial != branch);
    let implementation_commits_created = facts
        .initial_head
        .as_deref()
        .zip(facts.branch.as_deref())
        .and_then(|(initial_head, branch)| {
            git_count(project_root, &format!("{initial_head}..{branch}"))
        });
    let implementation_commits_ahead = facts
        .default_branch
        .as_deref()
        .zip(facts.branch.as_deref())
        .and_then(|(default_branch, branch)| {
            git_count(project_root, &format!("{default_branch}..{branch}"))
        });
    let implementation_diff_paths = facts
        .default_branch
        .as_deref()
        .zip(facts.branch.as_deref())
        .and_then(|(default_branch, branch)| {
            git_stdout(
                project_root,
                &[
                    "diff",
                    "--name-only",
                    &format!("{default_branch}...{branch}"),
                ],
            )
            .ok()
            .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count())
        });
    Dev2MergeObservedState {
        pr_bot_marker_exists,
        pr_state,
        pr_state_error,
        current_branch,
        current_head,
        branch_head,
        branch_moved,
        implementation_commits_ahead,
        implementation_commits_created,
        implementation_diff_empty: implementation_diff_paths.map(|count| count == 0),
        implementation_diff_paths,
    }
}

pub(super) fn evaluate_dev2merge_completion(
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
        (Some(_), Some(true)) => {}
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
    if !dev2merge_observed_side_effect(facts, observed) {
        failures.push(
            "ERROR: no dev2merge side effect was observed: branch did not move, no implementation commit was detected, no PR/merge side effect was verified, and no closed-issue no-op was recorded."
                .to_string(),
        );
    }
    if observed.implementation_diff_empty == Some(true)
        && !dev2merge_publish_side_effect_verified(facts, observed)
    {
        failures.push(format!(
            "ERROR: cumulative diff {}...{} is empty and no PR/issue/merge side effect was verified.",
            facts.default_branch.as_deref().unwrap_or("main"),
            facts.branch.as_deref().unwrap_or("HEAD"),
        ));
    }
    failures
}

fn dev2merge_observed_side_effect(
    facts: &Dev2MergeCompletionFacts,
    observed: &Dev2MergeObservedState,
) -> bool {
    observed.branch_moved == Some(true)
        || observed
            .implementation_commits_created
            .is_some_and(|count| count > 0)
        || observed
            .implementation_commits_ahead
            .is_some_and(|count| count > 0)
        || dev2merge_publish_side_effect_verified(facts, observed)
        || facts.already_resolved_skip
}

fn dev2merge_publish_side_effect_verified(
    facts: &Dev2MergeCompletionFacts,
    observed: &Dev2MergeObservedState,
) -> bool {
    facts.pr_number.is_some()
        && observed.pr_state.as_deref() == Some("MERGED")
        && observed.pr_bot_marker_exists == Some(true)
}

fn dev2merge_already_resolved_summary(facts: &Dev2MergeCompletionFacts) -> String {
    let mut parts = vec!["dev2merge: already-resolved no-op".to_string()];
    if let Some(branch) = &facts.branch {
        parts.push(format!("branch={branch}"));
    }
    if let Some(issue) = &facts.issue_number {
        parts.push(format!("issue=#{issue}"));
    }
    if let Some(message) = &facts.already_resolved_message {
        parts.push(format!("state={}", compact_summary_fragment(message)));
    }
    parts.push("next=none".to_string());
    parts.join("; ")
}

pub(super) fn dev2merge_success_summary(
    facts: &Dev2MergeCompletionFacts,
    observed: &Dev2MergeObservedState,
) -> String {
    let mut parts = vec!["dev2merge side effects verified".to_string()];
    if let Some(branch) = &facts.branch {
        parts.push(format!("branch={branch}"));
    }
    if let Some(current_branch) = &observed.current_branch {
        parts.push(format!("checkout={current_branch}"));
    }
    if let Some(head_moved) = observed.branch_moved {
        parts.push(format!("branch_moved={head_moved}"));
    }
    if let Some(count) = observed.implementation_commits_ahead {
        parts.push(format!("implementation_commits_ahead={count}"));
    }
    if let Some(count) = observed.implementation_commits_created {
        parts.push(format!("implementation_commits_created={count}"));
    }
    if let Some(paths) = observed.implementation_diff_paths {
        parts.push(format!("diff_paths={paths}"));
    }
    if let Some(current_head) = observed.current_head.as_deref().map(short_sha) {
        parts.push(format!("current_head={current_head}"));
    }
    if let Some(branch_head) = observed.branch_head.as_deref().map(short_sha) {
        parts.push(format!("branch_head={branch_head}"));
    }
    parts.push(format!("review_completed={}", facts.review_completed));
    parts.push(format!(
        "review_verdict_checked={}",
        facts.review_verdict_checked
    ));
    parts.push(format!("pushed={}", facts.pushed));
    if let Some(pr_number) = &facts.pr_number {
        parts.push(format!(
            "pr=#{pr_number} state={}",
            observed.pr_state.as_deref().unwrap_or("unknown")
        ));
    } else {
        parts.push("pr=none".to_string());
    }
    parts.push(match observed.pr_bot_marker_exists {
        Some(true) => "pr_bot_marker=present".to_string(),
        Some(false) => "pr_bot_marker=missing".to_string(),
        None => "pr_bot_marker=unknown".to_string(),
    });
    parts.push("next=none".to_string());
    parts.join("; ")
}

fn dev2merge_lifecycle_not_started_failure_report(
    workflow_name: &str,
    workflow_path: &Path,
    facts: &Dev2MergeCompletionFacts,
) -> PlanFailureError {
    let summary = "dev2merge lifecycle side-effect verification failed: publish gate never started"
        .to_string();
    let mut error = vec![
        "ERROR: dev2merge lifecycle side-effect verification failed after workflow steps reported success."
            .to_string(),
        "ERROR: Publish Gate (Step 13) did not run; branch publication, PR creation, pr-bot merge, and post-merge sync were not attempted."
            .to_string(),
    ];
    if facts.dev2merge_skip {
        error.push(
            "ERROR: DEV2MERGE_SKIP was true, but the Already-Resolved Check (Step 0) did not declare the skip in this run.".to_string(),
        );
    } else {
        error.push(
            "ERROR: DEV2MERGE_SKIP was not set by the Already-Resolved Check; terminal success would be transport-only.".to_string(),
        );
    }
    error.push(
        "Retry dev2merge from the current branch, or resume the journal if a manual handoff was expected; callers should treat this structured result as a missing lifecycle gate."
            .to_string(),
    );
    synthetic_failure(
        workflow_name,
        workflow_path,
        summary,
        "Dev2merge Lifecycle Side-Effect Verification",
        "post-run verifier: require Step 13-17 side effects or Step 0 already-resolved DEV2MERGE_SKIP=true",
        error,
    )
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
    synthetic_failure(
        workflow_name,
        workflow_path,
        summary,
        "Dev2merge Publish Side-Effect Verification",
        "post-run verifier: check Step 13-17 completion, PR_NUMBER, PR_BOT_DONE_MARKER, and gh PR state",
        error,
    )
}

fn synthetic_failure(
    workflow_name: &str,
    workflow_path: &Path,
    summary: String,
    title: &str,
    command: &str,
    error: Vec<String>,
) -> PlanFailureError {
    let error = error.join("\n");
    let synthetic_result = StepResult {
        step_id: DEV2MERGE_VERIFY_STEP_ID,
        title: title.to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(error.clone()),
        output: None,
        session_id: None,
        command: Some(command.to_string()),
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

pub(super) fn step_completed_successfully(
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

fn already_resolved_step_declared_skip(results: &[StepResult]) -> bool {
    already_resolved_skip_message(results).is_some()
}

fn already_resolved_skip_message(results: &[StepResult]) -> Option<String> {
    results.iter().find_map(|result| {
        if result.step_id != DEV2MERGE_ALREADY_RESOLVED_STEP_ID
            || result.skipped
            || result.exit_code != 0
        {
            return None;
        }
        result.output.as_deref().and_then(|output| {
            output
                .lines()
                .any(|line| line.trim() == "CSA_VAR:DEV2MERGE_SKIP=true")
                .then(|| {
                    output
                        .lines()
                        .map(str::trim)
                        .find(|line| {
                            !line.is_empty()
                                && !line.starts_with("CSA_VAR:")
                                && line.starts_with("dev2merge:")
                        })
                        .unwrap_or("already-resolved no-op")
                        .to_string()
                })
        })
    })
}

fn truthy(value: Option<&String>) -> bool {
    value
        .map(|value| {
            let lower = value.trim().to_ascii_lowercase();
            !lower.is_empty() && lower != "false" && lower != "0"
        })
        .unwrap_or(false)
}

fn numeric_var(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    non_empty_var(vars, key).filter(|value| value.chars().all(|ch| ch.is_ascii_digit()))
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

fn git_trimmed(project_root: &Path, args: &[&str]) -> Option<String> {
    git_stdout(project_root, args)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_stdout(project_root: &Path, args: &[&str]) -> Result<String> {
    command_stdout(project_root, "git", args).map_err(anyhow::Error::msg)
}

fn git_count(project_root: &Path, rev_range: &str) -> Option<usize> {
    git_stdout(project_root, &["rev-list", "--count", rev_range])
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
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

fn short_sha(value: &str) -> String {
    value.chars().take(12).collect()
}

fn compact_summary_fragment(value: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
