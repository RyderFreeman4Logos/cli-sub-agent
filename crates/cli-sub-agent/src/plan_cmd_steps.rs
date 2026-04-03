use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, bail};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use csa_hooks::format_next_step_directive;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep};

use super::plan_cmd_exec::{
    StepExecutionOutcome, execute_bash_step, execute_csa_step, run_with_heartbeat,
};
use super::{
    PlanRunJournal, apply_repo_fingerprint, detect_repo_fingerprint, persist_plan_journal,
    substitute_vars, validate_variable_name,
};

const OUTPUT_ASSIGNMENT_MARKER_PREFIX: &str = "CSA_VAR:";

/// Resolved execution target for a plan step.
///
/// Separates direct shell execution from AI tool dispatch so the routing
/// is type-safe — `tool = "bash"` can never accidentally fall through to
/// an AI tool's interactive confirmation flow.
pub(crate) enum StepTarget {
    /// Execute bash code block directly via `tokio::process::Command`.
    DirectBash,
    /// Skip this step (compile-time INCLUDE directive from weave).
    WeaveInclude,
    /// Dispatch to an AI tool via CSA infrastructure.
    CsaTool {
        tool_name: ToolName,
        model_spec: Option<String>,
    },
}

impl StepTarget {
    fn csa(tool: ToolName, spec: Option<String>) -> Self {
        Self::CsaTool {
            tool_name: tool,
            model_spec: spec,
        }
    }
}

/// Result of executing a single step.
pub(crate) struct StepResult {
    pub(crate) step_id: usize,
    pub(crate) title: String,
    pub(crate) exit_code: i32,
    pub(crate) duration_secs: f64,
    pub(crate) skipped: bool,
    pub(crate) error: Option<String>,
    /// Captured output from step execution (stdout for bash, output/summary for CSA).
    /// Available to subsequent steps as `${STEP_<id>_OUTPUT}`.
    pub(crate) output: Option<String>,
    /// CSA meta session ID produced by this step.
    /// Available to subsequent steps as `${STEP_<id>_SESSION}`.
    pub(crate) session_id: Option<String>,
}

pub(super) struct PlanRunContext<'a> {
    pub(super) project_root: &'a Path,
    pub(super) workflow_path: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) tool_override: Option<&'a ToolName>,
    pub(super) journal: &'a mut PlanRunJournal,
    pub(super) journal_path: Option<&'a Path>,
    pub(super) resume_completed_steps: &'a HashSet<usize>,
}

fn shell_escape_for_command(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn format_plan_resume_command(project_root: &Path, workflow_path: &Path) -> String {
    let display_path = workflow_path
        .strip_prefix(project_root)
        .unwrap_or(workflow_path);
    let display = display_path.to_string_lossy();
    format!(
        "csa plan run --sa-mode true {}",
        shell_escape_for_command(&display)
    )
}

/// Resolve a step's execution target from its annotations and config.
///
/// Resolution order:
/// 1. `step.tool` — explicit tool name (e.g. "bash", "claude-code", "codex")
/// 2. `step.tier` — tier name looked up in config's `tiers` map
/// 3. Fallback: "bash" (safest default for v1)
pub(crate) fn resolve_step_tool(
    step: &PlanStep,
    config: Option<&ProjectConfig>,
) -> Result<StepTarget> {
    // 1. Explicit tool annotation
    if let Some(ref tool_str) = step.tool {
        let tool_lower = tool_str.to_lowercase();
        match tool_lower.as_str() {
            "bash" => return Ok(StepTarget::DirectBash),
            "gemini-cli" => return Ok(StepTarget::csa(ToolName::GeminiCli, None)),
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
                                return Ok(StepTarget::csa(tool, Some(model_spec_str.clone())));
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
                "Unknown tool '{}' in step {} ('{}'). Known: bash, gemini-cli, opencode, codex, claude-code, csa, weave",
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
                        return Ok(StepTarget::csa(tool, Some(model_spec_str.clone())));
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

fn parse_tool_name(tool: &str) -> Result<ToolName> {
    match tool {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        other => bail!("Unknown tool: {other}"),
    }
}

/// Execute all steps in the plan sequentially.
///
/// After each successful step, injects `STEP_<id>_OUTPUT` into the variables
/// map so subsequent steps can reference prior outputs via `${STEP_1_OUTPUT}`.
#[cfg(test)]
pub(crate) async fn execute_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
) -> Result<Vec<StepResult>> {
    let workflow_path = project_root.join("workflow.toml");
    let mut journal = PlanRunJournal::new(&plan.name, &workflow_path, variables.clone());
    let completed = HashSet::new();
    let mut run_ctx = PlanRunContext {
        project_root,
        workflow_path: &workflow_path,
        config,
        tool_override,
        journal: &mut journal,
        journal_path: None,
        resume_completed_steps: &completed,
    };
    execute_plan_with_journal(plan, variables, &mut run_ctx).await
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

        let result = execute_step(
            step,
            &vars,
            run_ctx.project_root,
            run_ctx.config,
            run_ctx.tool_override,
        )
        .await;
        let is_failure = !result.skipped && result.exit_code != 0;

        // Inject step output for subsequent steps (successful steps only).
        let var_key = format!("STEP_{}_OUTPUT", result.step_id);
        let var_value = result.output.as_deref().unwrap_or("").to_string();
        let assignment_markers = if !is_failure && should_inject_assignment_markers(step) {
            extract_output_assignment_markers(&var_value, &assignment_marker_allowlist)
        } else {
            Vec::new()
        };
        vars.insert(var_key, var_value);
        let session_var_key = format!("STEP_{}_SESSION", result.step_id);
        let session_var_value = result.session_id.as_deref().unwrap_or("").to_string();
        vars.insert(session_var_key, session_var_value);
        for (key, value) in assignment_markers {
            vars.insert(key, value);
        }
        if !is_failure && !result.skipped {
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

        // Emit CSA:NEXT_STEP directive for pipeline chaining.
        // On success: point to the next step in the plan.
        // On failure: no directive (pipeline stops on abort).
        if !is_failure
            && !result.skipped
            && let Some(next_step) = find_next_step(step, &plan.steps)
        {
            let cmd = format_plan_resume_command(run_ctx.project_root, run_ctx.workflow_path);
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

/// Execute a single step with on_fail handling.
pub(crate) async fn execute_step(
    step: &PlanStep,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
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
    let target = match resolve_step_tool(step, config) {
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
    // Clear model_spec since the tier-resolved spec may reference a different tool.
    let target = if let Some(override_tool) = tool_override {
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
                }
            }
            other => other,
        }
    } else {
        target
    };

    // Skip weave INCLUDE steps (compile-time directive, not executable at runtime)
    if matches!(target, StepTarget::WeaveInclude) {
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
                    execute_bash_step(&label, &step.prompt, variables, project_root),
                    start,
                )
                .await
            }
            StepTarget::CsaTool {
                tool_name,
                model_spec,
            } => {
                let prompt = csa_prompt.as_deref().unwrap_or_default();
                run_with_heartbeat(
                    &label,
                    execute_csa_step(
                        &label,
                        prompt,
                        tool_name,
                        model_spec.as_deref(),
                        csa_session.as_deref(),
                        project_root,
                        config,
                    ),
                    start,
                )
                .await
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

pub(crate) fn extract_output_assignment_markers(
    output: &str,
    allowlist: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut markers = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let marker_payload = match trimmed.strip_prefix(OUTPUT_ASSIGNMENT_MARKER_PREFIX) {
            Some(payload) => payload.trim(),
            None => continue,
        };
        if let Some((raw_key, raw_value)) = marker_payload.split_once('=') {
            let key = raw_key.trim();
            if is_assignment_marker_key(key) && allowlist.contains(key) {
                markers.push((key.to_string(), raw_value.trim().to_string()));
            }
        }
    }
    markers
}

pub(crate) fn should_inject_assignment_markers(step: &PlanStep) -> bool {
    step.tool
        .as_deref()
        .is_some_and(|tool| tool.eq_ignore_ascii_case("bash"))
}

pub(crate) fn is_assignment_marker_key(key: &str) -> bool {
    validate_variable_name(key).is_ok()
}

/// Find the next step in the plan after the current step.
///
/// Returns the first step with an ID greater than the current step's ID,
/// which is the sequential successor in a linear workflow.
fn find_next_step<'a>(current: &PlanStep, steps: &'a [PlanStep]) -> Option<&'a PlanStep> {
    steps.iter().find(|s| s.id > current.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_plan_resume_command_uses_project_relative_path() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/patterns/dev2merge/workflow.toml");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path),
            "csa plan run --sa-mode true 'patterns/dev2merge/workflow.toml'"
        );
    }

    #[test]
    fn format_plan_resume_command_escapes_special_characters() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/patterns/weird name's/workflow.toml");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path),
            "csa plan run --sa-mode true 'patterns/weird name'\\''s/workflow.toml'"
        );
    }
}
