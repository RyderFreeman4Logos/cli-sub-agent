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
//! - Steps with `condition` or `loop_var` are skipped with a warning

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use csa_process::check_tool_installed;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep, plan_from_toml};

use crate::pipeline::{determine_project_root, execute_with_session};
use crate::run_helpers::build_executor;

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

    // 4. Load and parse workflow TOML
    let workflow_path = PathBuf::from(&file);
    if !workflow_path.exists() {
        bail!("Workflow file not found: {}", file);
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
    let total_start = Instant::now();
    let results = execute_plan(&plan, &variables, &project_root, config.as_ref()).await?;

    // 8. Print summary
    print_summary(&results, total_start.elapsed().as_secs_f64());

    // 9. Exit with error if any step failed (and wasn't skipped)
    let failed = results
        .iter()
        .filter(|r| !r.skipped && r.exit_code != 0)
        .count();
    if failed > 0 {
        bail!("{} step(s) failed", failed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Variable handling
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tier → tool resolution
// ---------------------------------------------------------------------------

/// Resolve a step's tool and model spec from its annotations and config.
///
/// Resolution order:
/// 1. `step.tool` — explicit tool name (e.g. "bash", "claude-code", "codex")
/// 2. `step.tier` — tier name looked up in config's `tiers` map
/// 3. Fallback: "bash" (safest default for v1)
fn resolve_step_tool(
    step: &PlanStep,
    config: Option<&ProjectConfig>,
) -> Result<(ToolName, Option<String>)> {
    // 1. Explicit tool annotation
    if let Some(ref tool_str) = step.tool {
        let tool_lower = tool_str.to_lowercase();
        match tool_lower.as_str() {
            "bash" => return Ok((ToolName::ClaudeCode, Some("bash".to_string()))),
            "gemini-cli" => return Ok((ToolName::GeminiCli, None)),
            "opencode" => return Ok((ToolName::Opencode, None)),
            "codex" => return Ok((ToolName::Codex, None)),
            "claude-code" => return Ok((ToolName::ClaudeCode, None)),
            // "csa" in v1: treat as "use default tool from config"
            "csa" => {
                if let Some(cfg) = config {
                    if let Some((_tool_name, model_spec)) = cfg.resolve_tier_tool("default") {
                        let spec = ModelSpec::parse(&model_spec)?;
                        let tool = parse_tool_name(&spec.tool)?;
                        return Ok((tool, Some(model_spec)));
                    }
                }
                // Fallback: codex as default CSA tool
                return Ok((ToolName::Codex, None));
            }
            other => bail!(
                "Unknown tool '{}' in step {} ('{}'). Known: bash, gemini-cli, opencode, codex, claude-code, csa",
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
                        return Ok((tool, Some(model_spec_str.clone())));
                    }
                }
            }
            warn!(
                "Tier '{}' not found or no enabled tools; falling back to codex for step {}",
                tier_name, step.id
            );
        }
        return Ok((ToolName::Codex, None));
    }

    // 3. Fallback: use default tool from config, or codex
    if let Some(cfg) = config {
        if let Some((_tool_name, model_spec)) = cfg.resolve_tier_tool("default") {
            let spec = ModelSpec::parse(&model_spec)?;
            let tool = parse_tool_name(&spec.tool)?;
            return Ok((tool, Some(model_spec)));
        }
    }

    Ok((ToolName::Codex, None))
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

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

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
        let should_abort = !result.skipped && result.exit_code != 0;

        // Check if we should abort the entire plan
        let abort = should_abort && matches!(step.on_fail, FailAction::Abort);
        results.push(result);

        if abort {
            error!(
                "Step {} ('{}') failed with on_fail=abort — stopping workflow",
                step.id, step.title
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

    // Skip steps with conditions or loops (v1 limitation)
    if step.condition.is_some() {
        warn!(
            "{} - Skipping: conditional steps not supported in v1",
            label
        );
        return StepResult {
            step_id: step.id,
            title: step.title.clone(),
            exit_code: 0,
            duration_secs: 0.0,
            skipped: true,
            error: Some("Conditional steps not supported in v1".to_string()),
        };
    }
    if step.loop_var.is_some() {
        warn!("{} - Skipping: loop steps not supported in v1", label);
        return StepResult {
            step_id: step.id,
            title: step.title.clone(),
            exit_code: 0,
            duration_secs: 0.0,
            skipped: true,
            error: Some("Loop steps not supported in v1".to_string()),
        };
    }

    // Substitute variables in prompt
    let prompt = substitute_vars(&step.prompt, variables);

    // Resolve tool
    let (tool_name, model_spec) = match resolve_step_tool(step, config) {
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

    // Determine retry count from on_fail
    let max_attempts = match &step.on_fail {
        FailAction::Retry(n) => (*n).max(1),
        _ => 1,
    };

    let mut last_result = None;

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!("{} - Retry attempt {}/{}", label, attempt, max_attempts);
        }

        let result = if model_spec.as_deref() == Some("bash") {
            // Direct bash execution
            execute_bash_step(&label, &prompt, project_root).await
        } else {
            // CSA tool execution
            execute_csa_step(
                &label,
                &prompt,
                &tool_name,
                model_spec.as_deref(),
                project_root,
                config,
            )
            .await
        };

        let exit_code = result.unwrap_or(1);

        if exit_code == 0 {
            info!(
                "{} - Completed in {:.2}s",
                label,
                start.elapsed().as_secs_f64()
            );
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

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
            Ok((tool, spec)) => {
                if let Some(s) = spec {
                    if s == "bash" {
                        "bash".to_string()
                    } else {
                        format!("{} ({})", tool.as_str(), s)
                    }
                } else {
                    tool.as_str().to_string()
                }
            }
            Err(e) => format!("<error: {}>", e),
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
mod tests {
    use super::*;
    use weave::compiler::VariableDecl;

    #[test]
    fn parse_variables_uses_defaults() {
        let plan = ExecutionPlan {
            name: "test".into(),
            description: String::new(),
            variables: vec![
                VariableDecl {
                    name: "FOO".into(),
                    default: Some("bar".into()),
                },
                VariableDecl {
                    name: "BAZ".into(),
                    default: None,
                },
            ],
            steps: vec![],
        };

        let vars = parse_variables(&[], &plan).unwrap();
        assert_eq!(vars.get("FOO").map(String::as_str), Some("bar"));
        assert!(!vars.contains_key("BAZ"));
    }

    #[test]
    fn parse_variables_cli_overrides_default() {
        let plan = ExecutionPlan {
            name: "test".into(),
            description: String::new(),
            variables: vec![VariableDecl {
                name: "FOO".into(),
                default: Some("default".into()),
            }],
            steps: vec![],
        };

        let vars = parse_variables(&["FOO=override".into()], &plan).unwrap();
        assert_eq!(vars.get("FOO").map(String::as_str), Some("override"));
    }

    #[test]
    fn parse_variables_rejects_invalid_format() {
        let plan = ExecutionPlan {
            name: "test".into(),
            description: String::new(),
            variables: vec![],
            steps: vec![],
        };

        let err = parse_variables(&["NO_EQUALS_SIGN".into()], &plan);
        assert!(err.is_err());
    }

    #[test]
    fn substitute_vars_replaces_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("NAME".into(), "world".into());
        vars.insert("COUNT".into(), "42".into());

        assert_eq!(
            substitute_vars("Hello ${NAME}, count=${COUNT}!", &vars),
            "Hello world, count=42!"
        );
    }

    #[test]
    fn substitute_vars_leaves_unknown_placeholders() {
        let vars = HashMap::new();
        assert_eq!(substitute_vars("${UNKNOWN}", &vars), "${UNKNOWN}");
    }

    #[test]
    fn extract_bash_code_block_finds_bash_fence() {
        let prompt = "Run this:\n```bash\necho hello\n```\nDone.";
        assert_eq!(extract_bash_code_block(prompt), Some("echo hello"));
    }

    #[test]
    fn extract_bash_code_block_finds_plain_fence() {
        let prompt = "```\nls -la\n```";
        assert_eq!(extract_bash_code_block(prompt), Some("ls -la"));
    }

    #[test]
    fn extract_bash_code_block_returns_none_when_no_fence() {
        assert_eq!(extract_bash_code_block("just some text"), None);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let s = "a".repeat(100);
        let result = truncate(&s, 10);
        assert_eq!(result.len(), 13); // 10 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn resolve_step_tool_explicit_bash() {
        let step = PlanStep {
            id: 1,
            title: "test".into(),
            tool: Some("bash".into()),
            prompt: String::new(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
        };
        let (tool, spec) = resolve_step_tool(&step, None).unwrap();
        assert_eq!(tool, ToolName::ClaudeCode);
        assert_eq!(spec.as_deref(), Some("bash"));
    }

    #[test]
    fn resolve_step_tool_explicit_codex() {
        let step = PlanStep {
            id: 1,
            title: "test".into(),
            tool: Some("codex".into()),
            prompt: String::new(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
        };
        let (tool, _) = resolve_step_tool(&step, None).unwrap();
        assert_eq!(tool, ToolName::Codex);
    }

    #[test]
    fn resolve_step_tool_fallback_no_config() {
        let step = PlanStep {
            id: 1,
            title: "test".into(),
            tool: None,
            prompt: String::new(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
        };
        let (tool, _) = resolve_step_tool(&step, None).unwrap();
        assert_eq!(tool, ToolName::Codex);
    }
}
