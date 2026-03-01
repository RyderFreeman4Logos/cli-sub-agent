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
//! - Workflow variables from `--var KEY=VALUE` and `STEP_<id>_OUTPUT`
//! - `${VAR}` substitution for CSA prompts and condition evaluation
//! - `on_fail` handling: abort / skip / retry N
//! - `condition` evaluation: `${VAR}` truthiness, `!(expr)`, `(a) && (b)`
//! - Steps with `loop_var` are skipped with a warning (v2)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep, plan_from_toml};

use crate::pipeline::determine_project_root;
use crate::plan_display::{print_plan, print_summary};

#[path = "plan_cmd_exec.rs"]
mod plan_cmd_exec;
use plan_cmd_exec::{
    StepExecutionOutcome, execute_bash_step, execute_csa_step, run_with_heartbeat,
};
#[cfg(test)]
use plan_cmd_exec::{extract_bash_code_block, is_stale_session_error, truncate};

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

const PLAN_JOURNAL_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanRunJournal {
    schema_version: u8,
    workflow_name: String,
    workflow_path: String,
    status: String,
    vars: HashMap<String, String>,
    completed_steps: Vec<usize>,
    last_error: Option<String>,
    #[serde(default)]
    repo_head: Option<String>,
    #[serde(default)]
    repo_dirty: Option<bool>,
}

impl PlanRunJournal {
    fn new(workflow_name: &str, workflow_path: &Path, vars: HashMap<String, String>) -> Self {
        Self {
            schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
            workflow_name: workflow_name.to_string(),
            workflow_path: normalize_path(workflow_path),
            status: "running".to_string(),
            vars,
            completed_steps: Vec::new(),
            last_error: None,
            repo_head: None,
            repo_dirty: None,
        }
    }
}

struct PlanResumeContext {
    initial_vars: HashMap<String, String>,
    completed_steps: HashSet<usize>,
    resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoFingerprint {
    head: Option<String>,
    dirty: Option<bool>,
}

fn normalize_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn safe_plan_name(plan_name: &str) -> String {
    let mut normalized: String = plan_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized.trim_matches('_').to_string()
}

fn plan_journal_path(project_root: &Path, plan_name: &str) -> PathBuf {
    let safe_name = safe_plan_name(plan_name);
    project_root
        .join(".csa")
        .join("state")
        .join("plan")
        .join(format!("{safe_name}.journal.json"))
}

fn persist_plan_journal(path: &Path, journal: &PlanRunJournal) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create plan state directory: {}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_vec_pretty(journal).context("Failed to encode plan journal")?;
    std::fs::write(path, encoded)
        .with_context(|| format!("Failed to write plan journal: {}", path.display()))?;
    Ok(())
}

fn detect_repo_fingerprint(project_root: &Path) -> RepoFingerprint {
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            } else {
                None
            }
        });

    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
            } else {
                None
            }
        });

    RepoFingerprint { head, dirty }
}

fn apply_repo_fingerprint(journal: &mut PlanRunJournal, fingerprint: &RepoFingerprint) {
    journal.repo_head = fingerprint.head.clone();
    journal.repo_dirty = fingerprint.dirty;
}

fn load_plan_resume_context(
    plan: &ExecutionPlan,
    workflow_path: &Path,
    journal_path: &Path,
    cli_vars: &HashMap<String, String>,
    repo_fingerprint: &RepoFingerprint,
) -> Result<PlanResumeContext> {
    let mut initial_vars = cli_vars.clone();
    if !journal_path.exists() {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            resumed: false,
        });
    }

    let bytes = std::fs::read(journal_path)
        .with_context(|| format!("Failed to read plan journal: {}", journal_path.display()))?;
    let journal: PlanRunJournal = serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse plan journal: {}", journal_path.display()))?;

    if journal.schema_version != PLAN_JOURNAL_SCHEMA_VERSION {
        warn!(
            path = %journal_path.display(),
            found = journal.schema_version,
            expected = PLAN_JOURNAL_SCHEMA_VERSION,
            "Ignoring plan journal with unsupported schema version"
        );
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            resumed: false,
        });
    }

    let same_workflow = journal.workflow_name == plan.name
        && journal.workflow_path == normalize_path(workflow_path);
    if !same_workflow || journal.status == "completed" {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            resumed: false,
        });
    }

    let fingerprint_matches = match (
        journal.repo_head.as_ref(),
        journal.repo_dirty,
        repo_fingerprint.head.as_ref(),
        repo_fingerprint.dirty,
    ) {
        (Some(saved_head), Some(saved_dirty), Some(current_head), Some(current_dirty)) => {
            saved_head == current_head && saved_dirty == current_dirty
        }
        _ => false,
    };
    if !fingerprint_matches {
        warn!(
            path = %journal_path.display(),
            "Ignoring plan journal because repository state changed (or fingerprint unavailable)"
        );
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            resumed: false,
        });
    }

    for (key, value) in journal.vars {
        initial_vars.insert(key, value);
    }
    // CLI-provided vars remain authoritative for declared variables.
    for (key, value) in cli_vars {
        initial_vars.insert(key.clone(), value.clone());
    }

    Ok(PlanResumeContext {
        initial_vars,
        completed_steps: journal.completed_steps.into_iter().collect(),
        resumed: true,
    })
}

fn detect_effective_repo(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // Strip credentials from HTTPS/SSH URLs (e.g. https://user:token@github.com/repo)
    let sanitized = if let Some(pos) = raw.find("://") {
        let (scheme, rest) = raw.split_at(pos + 3);
        if let Some(at_pos) = rest.find('@') {
            format!("{}{}", scheme, &rest[at_pos + 1..])
        } else {
            raw
        }
    } else {
        raw
    };

    let trimmed = sanitized.trim_end_matches(".git");
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return Some(rest.to_string());
    }
    Some(trimmed.to_string())
}

/// Handle `csa plan run <file>`.
pub(crate) async fn handle_plan_run(
    file: String,
    vars: Vec<String>,
    tool_override: Option<ToolName>,
    dry_run: bool,
    cd: Option<String>,
    current_depth: u32,
) -> Result<()> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;
    let effective_repo =
        detect_effective_repo(&project_root).unwrap_or_else(|| "(unknown)".to_string());
    eprintln!(
        "csa plan context: effective_repo={} effective_cwd={}",
        effective_repo,
        project_root.display()
    );

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
    let cli_variables = parse_variables(&vars, &plan)?;

    // 6. Dry-run: print plan and exit
    if dry_run {
        print_plan(&plan, &cli_variables, config.as_ref());
        return Ok(());
    }

    let journal_path = plan_journal_path(&project_root, &plan.name);
    let current_repo_fingerprint = detect_repo_fingerprint(&project_root);
    let resume_context = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &cli_variables,
        &current_repo_fingerprint,
    )?;
    if resume_context.resumed {
        let next_step = plan
            .steps
            .iter()
            .find(|step| !resume_context.completed_steps.contains(&step.id))
            .map(|step| step.id.to_string())
            .unwrap_or_else(|| "none".to_string());
        eprintln!(
            "Resuming workflow '{}' from journal (next step: {}).",
            plan.name, next_step
        );
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
    if let Some(ref tool) = tool_override {
        eprintln!("  Tool override: --tool {} (all CSA steps)", tool.as_str());
    }
    let mut journal = PlanRunJournal::new(
        &plan.name,
        &workflow_path,
        resume_context.initial_vars.clone(),
    );
    journal.completed_steps = resume_context.completed_steps.iter().copied().collect();
    apply_repo_fingerprint(&mut journal, &current_repo_fingerprint);
    persist_plan_journal(&journal_path, &journal)?;
    let mut run_ctx = PlanRunContext {
        project_root: &project_root,
        config: config.as_ref(),
        tool_override: tool_override.as_ref(),
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &resume_context.completed_steps,
    };

    let results =
        execute_plan_with_journal(&plan, &resume_context.initial_vars, &mut run_ctx).await?;

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
        journal.status = "failed".to_string();
        journal.last_error = Some(format!(
            "{} step(s) failed ({} execution, {} unsupported-skip)",
            total_failures, execution_failures, unsupported_skips
        ));
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        bail!(
            "{} step(s) failed ({} execution, {} unsupported-skip)",
            total_failures,
            execution_failures,
            unsupported_skips
        );
    }

    journal.status = "completed".to_string();
    journal.last_error = None;
    apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
    persist_plan_journal(&journal_path, &journal)?;

    Ok(())
}

// --- Variable handling ---

/// Parse `KEY=VALUE` pairs and merge with plan-declared defaults.
fn parse_variables(cli_vars: &[String], plan: &ExecutionPlan) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();

    // Seed with plan-declared defaults
    for decl in &plan.variables {
        validate_variable_name(&decl.name)?;
        if let Some(ref default) = decl.default {
            vars.insert(decl.name.clone(), default.clone());
        }
    }

    // Override with CLI --var values
    for entry in cli_vars {
        let (key, value) = entry
            .split_once('=')
            .with_context(|| format!("Invalid --var format '{}': expected KEY=VALUE", entry))?;
        validate_variable_name(key)?;
        vars.insert(key.to_string(), value.to_string());
    }

    Ok(vars)
}

/// Validate variable name format (`[A-Za-z_][A-Za-z0-9_]*`).
fn validate_variable_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid variable name '': must match [A-Za-z_][A-Za-z0-9_]*");
    };

    if !(first == '_' || first.is_ascii_alphabetic()) {
        bail!(
            "Invalid variable name '{}': must match [A-Za-z_][A-Za-z0-9_]*",
            name
        );
    }

    if chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        bail!(
            "Invalid variable name '{}': must match [A-Za-z_][A-Za-z0-9_]*",
            name
        );
    }
}

/// Substitute `${VAR}` placeholders in a string (used by CSA steps only).
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
///
/// After each successful step, injects `STEP_<id>_OUTPUT` into the variables
/// map so subsequent steps can reference prior outputs via `${STEP_1_OUTPUT}`.
#[cfg(test)]
async fn execute_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
) -> Result<Vec<StepResult>> {
    let mut journal = PlanRunJournal::new(&plan.name, Path::new("."), variables.clone());
    let completed = HashSet::new();
    let mut run_ctx = PlanRunContext {
        project_root,
        config,
        tool_override,
        journal: &mut journal,
        journal_path: None,
        resume_completed_steps: &completed,
    };
    execute_plan_with_journal(plan, variables, &mut run_ctx).await
}

struct PlanRunContext<'a> {
    project_root: &'a Path,
    config: Option<&'a ProjectConfig>,
    tool_override: Option<&'a ToolName>,
    journal: &'a mut PlanRunJournal,
    journal_path: Option<&'a Path>,
    resume_completed_steps: &'a HashSet<usize>,
}

async fn execute_plan_with_journal(
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
async fn execute_step(
    step: &PlanStep,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
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
                error: Some(format!("Tool resolution failed: {}", e)),
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
    if let Some(prompt) = csa_prompt.as_deref() {
        if prompt.trim().is_empty() {
            warn!(
                "{} - CSA step has empty prompt — tool will start with no context. \
                 This usually means a weave include was not expanded. \
                 Add a descriptive prompt to step {} in the workflow file.",
                label, step.id
            );
            eprintln!(
                "{} - WARNING: empty prompt for CSA step (tool will have no context)",
                label
            );
        }
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
            eprintln!("{} - RETRY {}/{}", label, attempt, max_attempts);
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
            eprintln!("{} - SKIP (exit {}, on_fail=skip)", label, exit_code);
            StepResult {
                step_id: step.id,
                title: step.title.clone(),
                exit_code,
                duration_secs: duration,
                skipped: true,
                error: Some(format!("Skipped after failure (exit code {})", exit_code)),
                output: None,
                session_id: None,
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
                output: None,
                session_id: None,
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
                output: None,
                session_id: None,
            }
        }
    }
}

fn extract_output_assignment_markers(
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

fn should_inject_assignment_markers(step: &PlanStep) -> bool {
    step.tool
        .as_deref()
        .is_some_and(|tool| tool.eq_ignore_ascii_case("bash"))
}

fn is_assignment_marker_key(key: &str) -> bool {
    validate_variable_name(key).is_ok()
}

#[cfg(test)]
#[path = "plan_cmd_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "plan_cmd_override_tests.rs"]
mod override_tests;
