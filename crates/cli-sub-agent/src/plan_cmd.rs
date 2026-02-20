//! Execute weave-compiled workflow files (`csa plan run`).
//!
//! This module bridges the gap between `weave compile` output (workflow.toml with
//! `[plan]`/`[[plan.steps]]` schema) and the CSA execution infrastructure.
//!
//! ## v1 Scope
//!
//! - Linear sequential execution of steps
//! - Tier→tool resolution via project config
//! - `tool = "bash"` direct execution (extracts code block from prompt)
//! - `${VAR}` substitution from `--var KEY=VALUE` CLI arguments
//! - `on_fail` handling: abort / skip / retry N
//! - `condition` evaluation: `${VAR}` truthiness, `!(expr)`, `(a) && (b)`
//! - Steps with `loop_var` are skipped with a warning (v2)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use csa_process::check_tool_installed;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep, plan_from_toml};

use crate::pipeline::{determine_project_root, execute_with_session};
use crate::run_helpers::build_executor;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

/// Resolved execution target for a plan step.
///
/// Separates direct shell execution from AI tool dispatch so the routing
/// is type-safe — `tool = "bash"` can never accidentally fall through to
/// an AI tool's interactive confirmation flow.
enum StepTarget {
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
struct StepResult {
    step_id: usize,
    title: String,
    exit_code: i32,
    duration_secs: f64,
    skipped: bool,
    error: Option<String>,
}

/// Handle `csa plan run <file>`.
pub(crate) async fn handle_plan_run(
    file: String,
    vars: Vec<String>,
    dry_run: bool,
    cd: Option<String>,
    current_depth: u32,
) -> Result<()> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Load project config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);
    if current_depth > max_depth {
        bail!(
            "Max recursion depth ({}) exceeded. Current: {}",
            max_depth,
            current_depth
        );
    }

    // 4. Load and parse workflow TOML (resolve relative to project root)
    let workflow_path = {
        let p = PathBuf::from(&file);
        if p.is_absolute() {
            p
        } else {
            project_root.join(&p)
        }
    };
    if !workflow_path.exists() {
        bail!("Workflow file not found: {}", workflow_path.display());
    }
    let content = std::fs::read_to_string(&workflow_path)
        .with_context(|| format!("Failed to read workflow file: {}", file))?;
    let plan = plan_from_toml(&content)
        .with_context(|| format!("Failed to parse workflow file: {}", file))?;

    // 5. Parse --var KEY=VALUE into HashMap
    let variables = parse_variables(&vars, &plan)?;

    // 6. Dry-run: print plan and exit
    if dry_run {
        print_plan(&plan, &variables, config.as_ref());
        return Ok(());
    }

    // 7. Execute steps sequentially
    info!(
        "Executing workflow '{}' ({} steps)",
        plan.name,
        plan.steps.len()
    );
    eprintln!(
        "Running workflow '{}' with {} step(s)...",
        plan.name,
        plan.steps.len()
    );
    let total_start = Instant::now();
    let results = execute_plan(&plan, &variables, &project_root, config.as_ref()).await?;

    // 8. Print summary
    print_summary(&results, total_start.elapsed().as_secs_f64());

    // 9. Warn about unsupported skips (loop_var)
    let unsupported_skips = results
        .iter()
        .filter(|r| r.skipped && r.exit_code != 0)
        .count();
    if unsupported_skips > 0 {
        warn!(
            "{} step(s) skipped due to unsupported v1 features (loops). \
             These steps were NOT executed — workflow results may be incomplete.",
            unsupported_skips
        );
    }

    // 10. Exit with error if any step failed (including unsupported skips,
    //     which use non-zero exit codes to prevent silent success).
    let execution_failures = results
        .iter()
        .filter(|r| !r.skipped && r.exit_code != 0)
        .count();
    let total_failures = execution_failures + unsupported_skips;
    if total_failures > 0 {
        bail!(
            "{} step(s) failed ({} execution, {} unsupported-skip)",
            total_failures,
            execution_failures,
            unsupported_skips
        );
    }

    Ok(())
}

// --- Variable handling ---

/// Parse `KEY=VALUE` pairs and merge with plan-declared defaults.
fn parse_variables(cli_vars: &[String], plan: &ExecutionPlan) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();

    // Seed with plan-declared defaults
    for decl in &plan.variables {
        if let Some(ref default) = decl.default {
            vars.insert(decl.name.clone(), default.clone());
        }
    }

    // Override with CLI --var values
    for entry in cli_vars {
        let (key, value) = entry
            .split_once('=')
            .with_context(|| format!("Invalid --var format '{}': expected KEY=VALUE", entry))?;
        vars.insert(key.to_string(), value.to_string());
    }

    Ok(vars)
}

/// Substitute `${VAR}` placeholders in a string.
fn substitute_vars(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("${{{}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

// --- Tier → tool resolution ---

/// Resolve a step's execution target from its annotations and config.
///
/// Resolution order:
/// 1. `step.tool` — explicit tool name (e.g. "bash", "claude-code", "codex")
/// 2. `step.tier` — tier name looked up in config's `tiers` map
/// 3. Fallback: "bash" (safest default for v1)
fn resolve_step_tool(step: &PlanStep, config: Option<&ProjectConfig>) -> Result<StepTarget> {
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
                    if let Some(ref tier_name) = step.tier {
                        if let Some(tier) = cfg.tiers.get(tier_name) {
                            for model_spec_str in &tier.models {
                                let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
                                if parts.len() == 4 && cfg.is_tool_enabled(parts[0]) {
                                    let tool = parse_tool_name(parts[0])?;
                                    return Ok(StepTarget::csa(tool, Some(model_spec_str.clone())));
                                }
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

    // 2. Tier annotation → look up in config
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
    if let Some(cfg) = config {
        if let Some((_tool_name, model_spec)) = cfg.resolve_tier_tool("default") {
            let spec = ModelSpec::parse(&model_spec)?;
            let tool = parse_tool_name(&spec.tool)?;
            return Ok(StepTarget::csa(tool, Some(model_spec)));
        }
    }

    Ok(StepTarget::csa(ToolName::Codex, None))
}

fn parse_tool_name(tool: &str) -> Result<ToolName> {
    match tool {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        other => bail!("Unknown tool: {}", other),
    }
}

// --- Execution ---

/// Execute all steps in the plan sequentially.
async fn execute_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<Vec<StepResult>> {
    let mut results = Vec::with_capacity(plan.steps.len());

    for step in &plan.steps {
        let result = execute_step(step, variables, project_root, config).await;
        let is_failure = !result.skipped && result.exit_code != 0;

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
            break;
        }
    }

    Ok(results)
}

/// Execute a single step with on_fail handling.
async fn execute_step(
    step: &PlanStep,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> StepResult {
    let start = Instant::now();
    let label = format!("[{}/{}]", step.id, step.title);
    eprintln!("{} - START", label);

    // Evaluate condition: skip step when condition evaluates to false.
    // Steps whose condition is true (or absent) proceed to execution.
    if let Some(ref condition) = step.condition {
        let condition_met = crate::plan_condition::evaluate_condition(condition, variables);
        if !condition_met {
            info!(
                "{} - SKIP (condition '{}' evaluated to false)",
                label, condition
            );
            eprintln!("{} - SKIP (condition not met)", label);
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: 0.0,
                skipped: true,
                error: None,
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
                error: Some(format!("Tool resolution failed: {}", e)),
            };
        }
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
        };
    }

    // Substitute variables in prompt
    let prompt = substitute_vars(&step.prompt, variables);

    // Determine retry count from on_fail
    let max_attempts = match &step.on_fail {
        FailAction::Retry(n) => (*n).max(1),
        _ => 1,
    };

    let mut last_result = None;

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!("{} - Retry attempt {}/{}", label, attempt, max_attempts);
            eprintln!("{} - RETRY {}/{}", label, attempt, max_attempts);
        }

        let exit_code = match &target {
            StepTarget::DirectBash => {
                run_with_heartbeat(
                    &label,
                    execute_bash_step(&label, &prompt, project_root),
                    start,
                )
                .await
            }
            StepTarget::CsaTool {
                tool_name,
                model_spec,
            } => {
                run_with_heartbeat(
                    &label,
                    execute_csa_step(
                        &label,
                        &prompt,
                        tool_name,
                        model_spec.as_deref(),
                        project_root,
                        config,
                    ),
                    start,
                )
                .await
            }
            StepTarget::WeaveInclude => unreachable!("handled above"),
        };

        if exit_code == 0 {
            info!(
                "{} - Completed in {:.2}s",
                label,
                start.elapsed().as_secs_f64()
            );
            eprintln!("{} - PASS ({:.2}s)", label, start.elapsed().as_secs_f64());
            return StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code: 0,
                duration_secs: start.elapsed().as_secs_f64(),
                skipped: false,
                error: None,
            };
        }

        last_result = Some(exit_code);
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
            eprintln!("{} - SKIP (exit {}, on_fail=skip)", label, exit_code);
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: true,
                error: Some(format!("Skipped after failure (exit code {})", exit_code)),
            }
        }
        FailAction::Delegate(target) => {
            warn!(
                "{} - Failed (exit {}), delegate to '{}' not supported in v1 — treating as abort",
                label, exit_code, target
            );
            eprintln!(
                "{} - FAIL (exit {}, delegate '{}' unsupported)",
                label, exit_code, target
            );
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: false,
                error: Some(format!(
                    "Delegate('{}') not supported in v1; step failed with exit code {}",
                    target, exit_code
                )),
            }
        }
        _ => {
            // Abort or Retry (already exhausted retries)
            error!("{} - Failed with exit code {}", label, exit_code);
            eprintln!("{} - FAIL (exit {})", label, exit_code);
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: false,
                error: Some(format!("Exit code {}", exit_code)),
            }
        }
    }
}

/// Keep workflow output alive for parents that enforce inactivity timeouts.
async fn run_with_heartbeat<F>(label: &str, execution: F, step_started_at: Instant) -> i32
where
    F: std::future::Future<Output = Result<i32>>,
{
    let mut execution = std::pin::pin!(execution);
    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            result = &mut execution => {
                return match result {
                    Ok(code) => code,
                    Err(err) => {
                        error!("{label} - Execution failed: {err}");
                        1
                    }
                };
            }
            _ = ticker.tick() => {
                eprintln!(
                    "{label} - RUNNING ({:.0}s elapsed)",
                    step_started_at.elapsed().as_secs_f64()
                );
            }
        }
    }
}

/// Execute a bash step by extracting the code block from the prompt.
async fn execute_bash_step(label: &str, prompt: &str, project_root: &Path) -> Result<i32> {
    let script = extract_bash_code_block(prompt).unwrap_or(prompt);
    info!("{} - Executing bash: {}", label, truncate(script, 80));

    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .current_dir(project_root)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .context("Failed to spawn bash")?;

    Ok(output.status.code().unwrap_or(1))
}

/// Execute a step via CSA tool (codex, claude-code, gemini-cli, opencode).
async fn execute_csa_step(
    label: &str,
    prompt: &str,
    tool_name: &ToolName,
    model_spec: Option<&str>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<i32> {
    info!("{} - Dispatching to {} ...", label, tool_name.as_str());

    // Build executor
    let executor = build_executor(tool_name, model_spec, None, None, config)?;

    // Check tool is installed
    check_tool_installed(executor.runtime_binary_name()).await?;

    // Load global config for env injection
    let global_config = csa_config::GlobalConfig::load()?;
    let extra_env = global_config.env_vars(executor.tool_name()).cloned();
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config, None);

    // Acquire global slot
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = csa_config::GlobalConfig::slots_dir()?;
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => slot,
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            bail!(
                "All {} slots for '{}' occupied ({}/{})",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            );
        }
        Err(e) => bail!(
            "Slot acquisition failed for '{}': {}",
            executor.tool_name(),
            e
        ),
    };

    // Execute with ephemeral session
    let result = execute_with_session(
        &executor,
        tool_name,
        prompt,
        None,                                 // session_arg: ephemeral
        Some("plan-step".to_string()),        // description
        std::env::var("CSA_SESSION_ID").ok(), // parent
        project_root,
        config,
        extra_env.as_ref(),
        Some("plan"),                         // task_type
        None,                                 // tier_name (already resolved)
        None,                                 // context_load_options
        csa_process::StreamMode::TeeToStderr, // stream for visibility
        idle_timeout_seconds,
        None, // MCP injection
    )
    .await?;

    Ok(result.exit_code)
}

// --- Helpers ---

/// Extract the first fenced code block from a prompt string.
///
/// Looks for ```bash or ``` blocks and returns the content.
fn extract_bash_code_block(prompt: &str) -> Option<&str> {
    // Find opening fence (```bash or ```)
    let start_patterns = ["```bash\n", "```sh\n", "```\n"];
    for pattern in &start_patterns {
        if let Some(start_idx) = prompt.find(pattern) {
            let code_start = start_idx + pattern.len();
            if let Some(end_idx) = prompt[code_start..].find("```") {
                let code = &prompt[code_start..code_start + end_idx];
                return Some(code.trim());
            }
        }
    }
    None
}

/// Truncate a string for display purposes.
fn truncate(s: &str, max_len: usize) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        first_line.to_string()
    }
}

// --- Display ---

/// Print the execution plan for dry-run mode.
fn print_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    config: Option<&ProjectConfig>,
) {
    println!("Workflow: {}", plan.name);
    if !plan.description.is_empty() {
        println!("  {}", plan.description);
    }
    println!();

    if !variables.is_empty() {
        println!("Variables:");
        for (k, v) in variables {
            println!("  ${{{k}}} = {v}");
        }
        println!();
    }

    println!("Steps ({}):", plan.steps.len());
    for step in &plan.steps {
        let tool_info = match resolve_step_tool(step, config) {
            Ok(StepTarget::DirectBash) => "bash (direct)".into(),
            Ok(StepTarget::WeaveInclude) => "weave (include)".into(),
            Ok(StepTarget::CsaTool {
                tool_name,
                model_spec,
            }) => match model_spec {
                Some(s) => format!("{} ({})", tool_name.as_str(), s),
                None => tool_name.as_str().to_string(),
            },
            Err(e) => format!("<error: {e}>"),
        };

        let on_fail = match &step.on_fail {
            FailAction::Abort => "abort",
            FailAction::Skip => "skip",
            FailAction::Retry(n) => &format!("retry({})", n),
            FailAction::Delegate(t) => &format!("delegate({})", t),
        };

        let flags = [
            step.condition.as_ref().map(|c| format!("IF {}", c)),
            step.loop_var
                .as_ref()
                .map(|l| format!("FOR {}", l.variable)),
        ];
        let flag_str: Vec<String> = flags.into_iter().flatten().collect();
        let flag_display = if flag_str.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flag_str.join(", "))
        };

        println!(
            "  {}. {} [tool={}, on_fail={}]{}",
            step.id, step.title, tool_info, on_fail, flag_display,
        );
    }
}

/// Print execution summary.
fn print_summary(results: &[StepResult], total_duration: f64) {
    println!();
    println!("=== Workflow Execution Summary ===");
    println!();

    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;

    for r in results {
        let status = if r.skipped {
            skip += 1;
            "- SKIP"
        } else if r.exit_code == 0 {
            pass += 1;
            "✓ PASS"
        } else {
            fail += 1;
            "✗ FAIL"
        };

        println!(
            "{:8} Step {} - {} ({:.2}s){}",
            status,
            r.step_id,
            r.title,
            r.duration_secs,
            r.error
                .as_ref()
                .map(|e| format!(" — {}", e))
                .unwrap_or_default(),
        );
    }

    println!();
    println!("Total: {} steps", results.len());
    println!("Passed: {pass}, Failed: {fail}, Skipped: {skip}");
    println!("Duration: {:.2}s", total_duration);
}

#[cfg(test)]
#[path = "plan_cmd_tests.rs"]
mod tests;
