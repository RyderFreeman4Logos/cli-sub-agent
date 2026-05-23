use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use csa_hooks::format_next_step_directive;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep};
use weave::parser::WorkspaceAccess;

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

/// Resolved execution target for a plan step.
/// Keeps direct shell execution separate from AI dispatch so `tool = "bash"` never falls through.
pub(crate) enum StepTarget {
    /// Execute bash code block directly via `tokio::process::Command`.
    DirectBash,
    /// Skip this step (compile-time INCLUDE directive from weave).
    WeaveInclude,
    /// Non-executable note for human-facing workflow context.
    Note,
    /// Manual action that must be handled by the orchestrator, not CSA.
    Manual,
    /// Stop the workflow and wait for explicit user action before any rerun.
    AwaitUser,
    /// Dispatch to an AI tool via CSA infrastructure.
    CsaTool {
        tool_name: ToolName,
        model_spec: Option<String>,
        tier_name: Option<String>,
    },
}

impl StepTarget {
    fn csa(tool: ToolName, spec: Option<String>) -> Self {
        Self::CsaTool {
            tool_name: tool,
            model_spec: spec,
            tier_name: None,
        }
    }

    fn csa_with_tier(tool: ToolName, spec: Option<String>, tier: String) -> Self {
        Self::CsaTool {
            tool_name: tool,
            model_spec: spec,
            tier_name: Some(tier),
        }
    }
}

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

/// Resolve a step target from its annotations and config.
/// Order: CLI overrides, explicit `step.tool`, then `step.tier`, then configured default or codex fallback.
pub(crate) fn resolve_step_tool(
    step: &PlanStep,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
    model_spec_override: Option<&String>,
) -> Result<StepTarget> {
    // 0. CLI overrides only apply to CSA-dispatched steps. Deterministic
    // workflow directives must remain deterministic even when --tool is set.
    let explicit_tool = step.tool.as_deref().map(str::to_ascii_lowercase);
    if let Some(tool) = tool_override
        && !matches!(
            explicit_tool.as_deref(),
            Some("bash" | "note" | "manual" | "await-user" | "weave")
        )
    {
        return Ok(StepTarget::csa(*tool, model_spec_override.cloned()));
    }

    // 1. Explicit tool annotation
    if let Some(tool_lower) = explicit_tool {
        match tool_lower.as_str() {
            "bash" => return Ok(StepTarget::DirectBash),
            "note" => return Ok(StepTarget::Note),
            "manual" => return Ok(StepTarget::Manual),
            "await-user" => return Ok(StepTarget::AwaitUser),
            "gemini-cli" => return Ok(StepTarget::csa(ToolName::GeminiCli, None)),
            "antigravity-cli" => return Ok(StepTarget::csa(ToolName::AntigravityCli, None)),
            "opencode" => return Ok(StepTarget::csa(ToolName::Opencode, None)),
            "codex" => return Ok(StepTarget::csa(ToolName::Codex, None)),
            "claude-code" => return Ok(StepTarget::csa(ToolName::ClaudeCode, None)),
            // "csa": use step.tier if present, else default tier from config
            "csa" => {
                if let Some(cfg) = config {
                    // Respect step.tier when tool=csa (P2 fix: don't ignore tier)
                    if let Some(ref tier_name) = step.tier
                        && let Some(tier) = cfg.tiers.get(tier_name)
                    {
                        for model_spec_str in &tier.models {
                            let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
                            if parts.len() == 4 && cfg.is_tool_enabled(parts[0]) {
                                let tool = parse_tool_name(parts[0])?;
                                return Ok(StepTarget::csa_with_tier(
                                    tool,
                                    Some(model_spec_str.clone()),
                                    tier_name.clone(),
                                ));
                            }
                        }
                    }
                    // Fallback: default tier
                    if let Some((_tool_name, model_spec)) = cfg.resolve_tier_tool("default") {
                        let spec = ModelSpec::parse(&model_spec)?;
                        let tool = parse_tool_name(&spec.tool)?;
                        return Ok(StepTarget::csa(tool, Some(model_spec)));
                    }
                }
                // Last resort: codex
                return Ok(StepTarget::csa(ToolName::Codex, None));
            }
            // "weave" = compile-time INCLUDE directive, skip at runtime
            "weave" => return Ok(StepTarget::WeaveInclude),
            other => bail!(
                "Unknown tool '{}' in step {} ('{}'). Known: bash, note, manual, await-user, gemini-cli, opencode, codex, claude-code, csa, weave",
                other,
                step.id,
                step.title
            ),
        }
    }

    // 2. Tier annotation -> look up in config
    if let Some(ref tier_name) = step.tier {
        if let Some(cfg) = config {
            if let Some(tier) = cfg.tiers.get(tier_name) {
                // Find first enabled tool in this tier
                for model_spec_str in &tier.models {
                    let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
                    if parts.len() == 4 && cfg.is_tool_enabled(parts[0]) {
                        let tool = parse_tool_name(parts[0])?;
                        return Ok(StepTarget::csa_with_tier(
                            tool,
                            Some(model_spec_str.clone()),
                            tier_name.clone(),
                        ));
                    }
                }
            }
            warn!(
                "Tier '{}' not found or no enabled tools; falling back to codex for step {}",
                tier_name, step.id
            );
        }
        return Ok(StepTarget::csa(ToolName::Codex, None));
    }

    // 3. Fallback: use default tool from config, or codex
    if let Some(cfg) = config
        && let Some((_tool_name, model_spec)) = cfg.resolve_tier_tool("default")
    {
        let spec = ModelSpec::parse(&model_spec)?;
        let tool = parse_tool_name(&spec.tool)?;
        return Ok(StepTarget::csa(tool, Some(model_spec)));
    }

    Ok(StepTarget::csa(ToolName::Codex, None))
}

pub(crate) fn step_readonly_project_root(step: &PlanStep) -> bool {
    matches!(step.workspace_access, Some(WorkspaceAccess::ReadOnly))
}

fn parse_tool_name(tool: &str) -> Result<ToolName> {
    match tool {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        other => bail!("Unknown tool: {other}"),
    }
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
        let orchestrator_handoff = orchestrator_handoff_mode(step);
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
                    model_spec: None,
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

    let mut last_result = None;

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
                    stderr: String::new(),
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

        last_result = Some(outcome.exit_code);
    }

    let exit_code = last_result.unwrap_or(1);
    let duration = start.elapsed().as_secs_f64();

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
                error: Some(format!("Skipped after failure (exit code {exit_code})")),
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
                    "Delegate('{target}') not supported in v1; step failed with exit code {exit_code}"
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
                error: Some(format!("Exit code {exit_code}")),
                output: None,
                session_id: None,
            }
        }
    }
}
