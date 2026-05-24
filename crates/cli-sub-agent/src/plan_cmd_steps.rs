use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_hooks::format_next_step_directive;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep};

use super::plan_cmd_assignment::{
    extract_output_assignment_markers, should_inject_assignment_markers,
    strip_assignment_marker_lines,
};
use super::plan_cmd_exec::{StepExecutionOutcome, execute_bash_step, run_with_heartbeat};
use super::plan_cmd_flow::{
    OrchestratorHandoff, find_next_step, format_orchestrator_message, format_plan_resume_command,
    orchestrator_handoff_mode,
};
use super::plan_cmd_tier_failover::{TierFailoverParams, execute_csa_step_with_tier_failover};
use super::{
    PlanRunJournal, apply_repo_fingerprint, detect_repo_fingerprint, persist_plan_journal,
    substitute_vars,
};

const STEP_FAILURE_STDERR_TAIL_LINES: usize = 20;
const STEP_FAILURE_STDERR_TAIL_MAX_CHARS: usize = 4000;

#[path = "plan_cmd_step_target.rs"]
mod step_target;
pub(crate) use step_target::{StepTarget, resolve_step_tool, step_readonly_project_root};

/// Result of executing a single step.
#[derive(Serialize, Deserialize)]
pub(crate) struct StepResult {
    pub(crate) step_id: usize,
    pub(crate) title: String,
    pub(crate) exit_code: i32,
    pub(crate) duration_secs: f64,
    pub(crate) skipped: bool,
    pub(crate) error: Option<String>,
    /// Captured step output, exposed to later steps as `${STEP_<id>_OUTPUT}`.
    pub(crate) output: Option<String>,
    /// CSA meta session ID, exposed to later steps as `${STEP_<id>_SESSION}`.
    pub(crate) session_id: Option<String>,
}

pub(super) struct PlanRunContext<'a> {
    pub(super) project_root: &'a Path,
    pub(super) workflow_path: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) tool_override: Option<&'a ToolName>,
    pub(super) model_spec_override: Option<&'a String>,
    pub(super) journal: &'a mut PlanRunJournal,
    pub(super) journal_path: Option<&'a Path>,
    pub(super) resume_completed_steps: &'a HashSet<usize>,
    pub(super) chunked: bool,
    pub(super) no_fs_sandbox: bool,
}

pub(crate) struct StepExecutionContext<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) workflow_path: &'a Path,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) tool_override: Option<&'a ToolName>,
    pub(crate) model_spec_override: Option<&'a String>,
    pub(crate) no_fs_sandbox: bool,
}

pub(super) async fn execute_plan_with_journal(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    run_ctx: &mut PlanRunContext<'_>,
) -> Result<Vec<StepResult>> {
    let mut results = Vec::with_capacity(plan.steps.len());
    let mut vars = variables.clone();
    let mut completed_steps = run_ctx.resume_completed_steps.clone();
    let assignment_marker_allowlist: HashSet<String> = plan
        .variables
        .iter()
        .map(|decl| decl.name.clone())
        .collect();

    run_ctx.journal.status = "running".to_string();
    run_ctx.journal.vars = vars.clone();
    run_ctx.journal.completed_steps = completed_steps.iter().copied().collect();
    run_ctx.journal.last_error = None;
    apply_repo_fingerprint(
        run_ctx.journal,
        &detect_repo_fingerprint(run_ctx.project_root),
    );
    if let Some(path) = run_ctx.journal_path {
        persist_plan_journal(path, run_ctx.journal)?;
    }

    for step in &plan.steps {
        if completed_steps.contains(&step.id) {
            eprintln!(
                "[{}/{}] - RESUME-SKIP (already completed)",
                step.id, step.title
            );
            continue;
        }

        let result = execute_step_with_workflow(
            step,
            &vars,
            &StepExecutionContext {
                project_root: run_ctx.project_root,
                workflow_path: run_ctx.workflow_path,
                config: run_ctx.config,
                tool_override: run_ctx.tool_override,
                model_spec_override: run_ctx.model_spec_override,
                no_fs_sandbox: run_ctx.no_fs_sandbox,
            },
        )
        .await;
        let orchestrator_handoff = if result.skipped {
            None
        } else {
            orchestrator_handoff_mode(step)
        };
        let is_failure = !result.skipped && result.exit_code != 0;

        // Inject step output for subsequent steps (successful steps only).
        let var_key = format!("STEP_{}_OUTPUT", result.step_id);
        let raw_output = result.output.as_deref().unwrap_or("").to_string();
        let assignment_markers = if !is_failure && should_inject_assignment_markers(step) {
            extract_output_assignment_markers(&raw_output, &assignment_marker_allowlist)
        } else {
            Vec::new()
        };
        // Strip CSA_VAR: lines so downstream variable references keep clean step output.
        let var_value = strip_assignment_marker_lines(&raw_output);
        vars.insert(var_key, var_value);
        let session_var_key = format!("STEP_{}_SESSION", result.step_id);
        let session_var_value = result.session_id.as_deref().unwrap_or("").to_string();
        vars.insert(session_var_key, session_var_value);
        for (key, value) in assignment_markers {
            vars.insert(key, value);
        }
        let is_manual_handoff = matches!(
            orchestrator_handoff,
            Some(OrchestratorHandoff::ManualResume)
        );
        if !is_failure && !is_manual_handoff {
            // Record executed/skipped steps as completed so --resume does not re-evaluate them.
            // Manual handoff only prints instructions, so explicit resume must replay it.
            completed_steps.insert(step.id);
        }
        run_ctx.journal.vars = vars.clone();
        run_ctx.journal.completed_steps = completed_steps.iter().copied().collect();
        apply_repo_fingerprint(
            run_ctx.journal,
            &detect_repo_fingerprint(run_ctx.project_root),
        );
        if let Some(path) = run_ctx.journal_path {
            persist_plan_journal(path, run_ctx.journal)?;
        }

        if let Some(handoff_mode) = orchestrator_handoff {
            run_ctx.journal.status = match handoff_mode {
                OrchestratorHandoff::ManualResume => "manual-handoff".to_string(),
                OrchestratorHandoff::AwaitUser => "awaiting-user".to_string(),
            };
            run_ctx.journal.last_error = result.error.clone();
            apply_repo_fingerprint(
                run_ctx.journal,
                &detect_repo_fingerprint(run_ctx.project_root),
            );
            if let Some(path) = run_ctx.journal_path {
                persist_plan_journal(path, run_ctx.journal)?;
            }
            if run_ctx.chunked {
                let json = serde_json::to_string(&result)
                    .expect("StepResult serialization should never fail");
                println!("{json}");
                results.push(result);
                break;
            }
            if let Some(output) = result.output.as_deref() {
                println!("{output}");
            }
            if matches!(handoff_mode, OrchestratorHandoff::ManualResume)
                && let Some(next_step) = find_next_step(step, &plan.steps)
            {
                let cmd = format_plan_resume_command(
                    run_ctx.project_root,
                    run_ctx.workflow_path,
                    run_ctx.journal_path,
                );
                let required = matches!(next_step.on_fail, FailAction::Abort);
                println!("MANUAL_STEP_RESUME: {cmd}");
                eprintln!("{}", format_next_step_directive(&cmd, required));
            }
            results.push(result);
            break;
        }

        // Chunked mode: emit the single StepResult as JSON to stdout and stop.
        // Skipped steps (condition-false) do not count as "executed" — continue
        // to the next step so the caller gets a real execution per chunk.
        if run_ctx.chunked && !result.skipped {
            let json =
                serde_json::to_string(&result).expect("StepResult serialization should never fail");
            println!("{json}");
            results.push(result);
            break;
        }

        // Emit CSA:NEXT_STEP directive for pipeline chaining.
        // On success: point to the next step in the plan.
        // On failure: no directive (pipeline stops on abort).
        if !is_failure
            && !result.skipped
            && let Some(next_step) = find_next_step(step, &plan.steps)
        {
            let cmd = format_plan_resume_command(
                run_ctx.project_root,
                run_ctx.workflow_path,
                run_ctx.journal_path,
            );
            let required = matches!(next_step.on_fail, FailAction::Abort);
            eprintln!("{}", format_next_step_directive(&cmd, required));
        }

        // Abort on failure when: on_fail=abort, or retry exhausted (retries
        // already happened inside execute_step; reaching here means all failed),
        // or delegate (unsupported in v1 — treated as abort).
        let abort = is_failure
            && matches!(
                step.on_fail,
                FailAction::Abort | FailAction::Retry(_) | FailAction::Delegate(_)
            );
        results.push(result);

        if abort {
            error!(
                "Step {} ('{}') failed (on_fail={:?}) — stopping workflow",
                step.id, step.title, step.on_fail
            );
            run_ctx.journal.status = "failed".to_string();
            run_ctx.journal.last_error = Some(format!(
                "Step {} ('{}') failed with on_fail={:?}",
                step.id, step.title, step.on_fail
            ));
            apply_repo_fingerprint(
                run_ctx.journal,
                &detect_repo_fingerprint(run_ctx.project_root),
            );
            if let Some(path) = run_ctx.journal_path {
                persist_plan_journal(path, run_ctx.journal)?;
            }
            break;
        }
    }

    Ok(results)
}

pub(crate) async fn execute_step_with_workflow(
    step: &PlanStep,
    variables: &HashMap<String, String>,
    step_ctx: &StepExecutionContext<'_>,
) -> StepResult {
    let start = Instant::now();
    let label = format!("[{}/{}]", step.id, step.title);
    eprintln!("{label} - START");

    // Evaluate condition: skip step when condition evaluates to false.
    // Steps whose condition is true (or absent) proceed to execution.
    if let Some(ref condition) = step.condition {
        let condition_met = crate::plan_condition::evaluate_condition(condition, variables);
        if !condition_met {
            info!(
                "{} - SKIP (condition '{}' evaluated to false)",
                label, condition
            );
            eprintln!("{label} - SKIP (condition not met)");
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: 0.0,
                skipped: true,
                error: None,
                output: None,
                session_id: None,
            };
        }
        info!("{} - Condition '{}' met, proceeding", label, condition);
    }
    if step.loop_var.is_some() {
        warn!("{} - UNSUPPORTED: loop steps require v2; skipping", label);
        return StepResult {
            step_id: step.id,
            title: step.title.clone(),
            exit_code: 2,
            duration_secs: 0.0,
            skipped: true,
            error: Some("Loop steps not supported in v1".to_string()),
            output: None,
            session_id: None,
        };
    }

    // Resolve execution target (needed for weave-include check)
    let target = match resolve_step_tool(
        step,
        step_ctx.config,
        step_ctx.tool_override,
        step_ctx.model_spec_override,
    ) {
        Ok(t) => t,
        Err(e) => {
            error!("{} - Failed to resolve tool: {}", label, e);
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                skipped: false,
                error: Some(format!("Tool resolution failed: {e}")),
                output: None,
                session_id: None,
            };
        }
    };

    // Apply --tool override: replace tool for all CSA steps (bash/weave unaffected).
    // Clear model_spec and tier_name since the override bypasses tier routing.
    let target = if let Some(override_tool) = step_ctx.tool_override {
        match target {
            StepTarget::CsaTool { .. } => {
                info!(
                    "{} - Tool override: {} → {}",
                    label,
                    "tier-resolved",
                    override_tool.as_str()
                );
                StepTarget::CsaTool {
                    tool_name: *override_tool,
                    model_spec: step_ctx.model_spec_override.cloned(),
                    tier_name: None,
                }
            }
            other => other,
        }
    } else {
        target
    };

    match target {
        StepTarget::WeaveInclude => {
            info!("{} - Skipping INCLUDE step (compile-time directive)", label);
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: 0.0,
                skipped: true,
                error: None,
                output: None,
                session_id: None,
            };
        }
        StepTarget::Note => {
            info!("{} - NOTE step (non-executable)", label);
            eprintln!("{label} - NOTE");
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: 0.0,
                skipped: true,
                error: None,
                output: None,
                session_id: None,
            };
        }
        StepTarget::Manual => {
            let message = format_orchestrator_message(step, OrchestratorHandoff::ManualResume);
            let status = format!(
                "Manual handoff required for step '{}'; execute the documented main-agent action, then resume the workflow explicitly.",
                step.title
            );
            warn!("{} - {}", label, status);
            eprintln!("{label} - MANUAL ACTION REQUIRED");
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: start.elapsed().as_secs_f64(),
                skipped: false,
                error: Some(status),
                output: Some(message),
                session_id: None,
            };
        }
        StepTarget::AwaitUser => {
            let message = format_orchestrator_message(step, OrchestratorHandoff::AwaitUser);
            let status = format!(
                "Awaiting user action for step '{}'; rerun the workflow from the beginning after the remediation is complete.",
                step.title
            );
            warn!("{} - {}", label, status);
            eprintln!("{label} - AWAITING USER ACTION");
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: start.elapsed().as_secs_f64(),
                skipped: false,
                error: Some(status),
                output: Some(message),
                session_id: None,
            };
        }
        StepTarget::DirectBash | StepTarget::CsaTool { .. } => {}
    }

    // CSA prompts use template substitution. Bash steps receive variables via env vars.
    let csa_prompt = match &target {
        StepTarget::CsaTool { .. } => Some(substitute_vars(&step.prompt, variables)),
        _ => None,
    };
    let csa_session = match &target {
        StepTarget::CsaTool { .. } => step
            .session
            .as_deref()
            .map(|session| substitute_vars(session, variables))
            .and_then(|session| {
                let trimmed = session.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }),
        _ => None,
    };

    // Warn when a CSA step has an empty prompt (likely a missing weave include)
    if let Some(prompt) = csa_prompt.as_deref()
        && prompt.trim().is_empty()
    {
        warn!(
            "{} - CSA step has empty prompt — tool will start with no context. \
                 This usually means a weave include was not expanded. \
                 Add a descriptive prompt to step {} in the workflow file.",
            label, step.id
        );
        eprintln!("{label} - WARNING: empty prompt for CSA step (tool will have no context)");
    }

    // Determine retry count from on_fail
    let max_attempts = match &step.on_fail {
        FailAction::Retry(n) => (*n).max(1),
        _ => 1,
    };

    let mut last_failure = None;

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!("{} - Retry attempt {}/{}", label, attempt, max_attempts);
            eprintln!("{label} - RETRY {attempt}/{max_attempts}");
        }

        let execution_result = match &target {
            StepTarget::DirectBash => {
                run_with_heartbeat(
                    &label,
                    execute_bash_step(
                        &label,
                        &step.prompt,
                        variables,
                        step_ctx.project_root,
                        step_ctx.workflow_path,
                    ),
                    start,
                )
                .await
            }
            StepTarget::CsaTool {
                tool_name,
                model_spec,
                tier_name,
            } => {
                let prompt = csa_prompt.as_deref().unwrap_or_default();
                let readonly_project_root = step_readonly_project_root(step);
                execute_csa_step_with_tier_failover(
                    &label,
                    prompt,
                    &TierFailoverParams {
                        initial_tool: tool_name,
                        initial_model_spec: model_spec.as_deref(),
                        tier_name: tier_name.as_deref(),
                        forwarded_session: csa_session.as_deref(),
                        readonly_project_root,
                    },
                    step_ctx,
                    start,
                )
                .await
            }
            StepTarget::Note | StepTarget::Manual | StepTarget::AwaitUser => {
                unreachable!("handled above")
            }
            StepTarget::WeaveInclude => unreachable!("handled above"),
        };
        let outcome = match execution_result {
            Ok(outcome) => outcome,
            Err(err) => {
                error!("{label} - Execution failed: {err}");
                StepExecutionOutcome {
                    exit_code: 1,
                    output: String::new(),
                    session_id: None,
                    stderr: format!("{err:#}"),
                }
            }
        };

        if outcome.exit_code == 0 {
            info!(
                "{} - Completed in {:.2}s",
                label,
                start.elapsed().as_secs_f64()
            );
            eprintln!("{} - PASS ({:.2}s)", label, start.elapsed().as_secs_f64());
            let output = if outcome.output.is_empty() {
                None
            } else {
                Some(outcome.output)
            };
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: start.elapsed().as_secs_f64(),
                skipped: false,
                error: None,
                output,
                session_id: outcome.session_id,
            };
        }

        last_failure = Some(outcome);
    }

    let exit_code = last_failure
        .as_ref()
        .map(|outcome| outcome.exit_code)
        .unwrap_or(1);
    let duration = start.elapsed().as_secs_f64();
    let failure_stderr = last_failure
        .as_ref()
        .map(|outcome| outcome.stderr.as_str())
        .unwrap_or_default();
    let failure_error = format_step_failure_error(exit_code, failure_stderr);

    // Handle on_fail
    match &step.on_fail {
        FailAction::Skip => {
            warn!(
                "{} - Failed (exit {}), skipping per on_fail=skip",
                label, exit_code
            );
            eprintln!("{label} - SKIP (exit {exit_code}, on_fail=skip)");
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: true,
                error: Some(format!("Skipped after failure ({failure_error})")),
                output: None,
                session_id: None,
            }
        }
        FailAction::Delegate(target) => {
            warn!(
                "{} - Failed (exit {}), delegate to '{}' not supported in v1 — treating as abort",
                label, exit_code, target
            );
            eprintln!("{label} - FAIL (exit {exit_code}, delegate '{target}' unsupported)");
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: false,
                error: Some(format!(
                    "Delegate('{target}') not supported in v1; step failed with {failure_error}"
                )),
                output: None,
                session_id: None,
            }
        }
        _ => {
            // Abort or Retry (already exhausted retries)
            error!("{} - Failed with exit code {}", label, exit_code);
            eprintln!("{label} - FAIL (exit {exit_code})");
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: false,
                error: Some(failure_error),
                output: None,
                session_id: None,
            }
        }
    }
}

fn format_step_failure_error(exit_code: i32, stderr: &str) -> String {
    let mut error = format!("Exit code {exit_code}");
    if let Some(stderr_tail) = stderr_tail(stderr) {
        error.push_str(&format!(
            "\nstderr (last {STEP_FAILURE_STDERR_TAIL_LINES} lines):\n{stderr_tail}"
        ));
    }
    error
}

fn stderr_tail(stderr: &str) -> Option<String> {
    let mut lines = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(STEP_FAILURE_STDERR_TAIL_LINES)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let mut tail = lines.join("\n");
    if tail.len() > STEP_FAILURE_STDERR_TAIL_MAX_CHARS {
        let keep_from = tail
            .char_indices()
            .rev()
            .find_map(|(idx, _)| {
                (tail.len() - idx <= STEP_FAILURE_STDERR_TAIL_MAX_CHARS).then_some(idx)
            })
            .unwrap_or(0);
        tail = format!("...{}", &tail[keep_from..]);
    }
    Some(tail)
}
