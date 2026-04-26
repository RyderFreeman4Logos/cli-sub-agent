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
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use weave::compiler::{ExecutionPlan, plan_from_toml};

use crate::pattern_resolver;
use crate::pipeline::determine_project_root;
use crate::plan_display::{print_plan, print_summary};

#[path = "plan_cmd_exec.rs"]
mod plan_cmd_exec;
#[cfg(test)]
use plan_cmd_exec::{extract_bash_code_block, truncate};

#[path = "plan_cmd_flow.rs"]
mod plan_cmd_flow;

#[path = "plan_cmd_steps.rs"]
mod plan_cmd_steps;
pub(crate) use plan_cmd_flow::shell_escape_for_command;
use plan_cmd_steps::{PlanRunContext, execute_plan_with_journal};
pub(crate) use plan_cmd_steps::{StepResult, StepTarget, resolve_step_tool};
#[cfg(test)]
pub(crate) use plan_cmd_steps::{
    execute_plan, execute_step, extract_output_assignment_markers, is_assignment_marker_key,
    should_inject_assignment_markers,
};

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
    explicit_resume: bool,
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
    let status_prevents_resume = matches!(
        journal.status.as_str(),
        "completed" | "awaiting-user" | "manual-handoff"
    );
    if !same_workflow
        || status_prevents_resume && !(explicit_resume && journal.status == "manual-handoff")
    {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            resumed: false,
        });
    }

    if !explicit_resume {
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
    } else {
        info!(
            path = %journal_path.display(),
            "Explicit --resume: bypassing repository fingerprint check"
        );
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

/// Resolve a workflow TOML path from either a file path or a pattern name.
fn resolve_workflow_path(
    file: &Option<String>,
    pattern: &Option<String>,
    project_root: &Path,
) -> Result<PathBuf> {
    match (file, pattern) {
        (Some(f), None) => {
            let p = PathBuf::from(f);
            let resolved = if p.is_absolute() {
                p
            } else {
                project_root.join(&p)
            };
            if !resolved.exists() {
                bail!("Workflow file not found: {}", resolved.display());
            }
            Ok(resolved)
        }
        (None, Some(name)) => {
            let resolved = pattern_resolver::resolve_pattern(name, project_root)?;
            let workflow = resolved.dir.join("workflow.toml");
            if !workflow.exists() {
                bail!(
                    "Pattern '{}' resolved to {} but no workflow.toml found there",
                    name,
                    resolved.dir.display()
                );
            }
            eprintln!(
                "csa plan: resolved --pattern {} → {}",
                name,
                workflow.display()
            );
            Ok(workflow)
        }
        (Some(_), Some(_)) => bail!("Cannot specify both FILE and --pattern"),
        (None, None) => bail!("Either FILE or --pattern is required"),
    }
}

/// CLI arguments for `csa plan run`.
pub(crate) struct PlanRunArgs {
    pub file: Option<String>,
    pub pattern: Option<String>,
    pub vars: Vec<String>,
    pub tool_override: Option<ToolName>,
    pub dry_run: bool,
    pub chunked: bool,
    pub resume: Option<String>,
    pub cd: Option<String>,
    pub current_depth: u32,
}

/// Handle `csa plan run <file>` or `csa plan run --pattern <name>`.
///
/// Foreground entry point: runs the workflow synchronously in the calling
/// process. Used by `--foreground` callers, `--dry-run`, `--chunked`,
/// `--resume`, and the daemon-child path (after env priming).
pub(crate) async fn handle_plan_run(args: PlanRunArgs) -> Result<()> {
    run_workflow_inline(args).await
}

async fn run_workflow_inline(args: PlanRunArgs) -> Result<()> {
    let PlanRunArgs {
        file,
        pattern,
        vars,
        tool_override,
        dry_run,
        chunked,
        resume,
        cd,
        current_depth,
    } = args;

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
        bail!("Max recursion depth ({max_depth}) exceeded. Current: {current_depth}");
    }

    // 4. Load workflow: either from --resume journal or from file/pattern
    let (workflow_path, plan, journal_path, explicit_resume) = if let Some(ref resume_path) = resume
    {
        // --resume: load journal state and extract workflow path from it
        let resume_file = PathBuf::from(resume_path);
        if !resume_file.exists() {
            bail!("Resume journal file not found: {}", resume_file.display());
        }
        let bytes = std::fs::read(&resume_file)
            .with_context(|| format!("Failed to read resume journal: {}", resume_file.display()))?;
        let journal: PlanRunJournal = serde_json::from_slice(&bytes).with_context(|| {
            format!("Failed to parse resume journal: {}", resume_file.display())
        })?;
        if journal.schema_version != PLAN_JOURNAL_SCHEMA_VERSION {
            bail!(
                "Resume journal has unsupported schema version {} (expected {})",
                journal.schema_version,
                PLAN_JOURNAL_SCHEMA_VERSION
            );
        }
        let wf_path = PathBuf::from(&journal.workflow_path);
        if !wf_path.exists() {
            bail!(
                "Workflow file from resume journal not found: {}",
                wf_path.display()
            );
        }
        let content = std::fs::read_to_string(&wf_path).with_context(|| {
            format!(
                "Failed to read workflow file from resume journal: {}",
                wf_path.display()
            )
        })?;
        let loaded_plan = plan_from_toml(&content).with_context(|| {
            format!(
                "Failed to parse workflow file from resume journal: {}",
                wf_path.display()
            )
        })?;
        eprintln!(
            "csa plan: --resume from journal {} (workflow: {})",
            resume_file.display(),
            wf_path.display()
        );
        (wf_path, loaded_plan, resume_file, true)
    } else {
        // Normal flow: resolve by file path or pattern name
        let wf_path = resolve_workflow_path(&file, &pattern, &project_root)?;
        let display_name = pattern
            .as_deref()
            .unwrap_or_else(|| file.as_deref().unwrap_or("(unknown)"));
        let content = std::fs::read_to_string(&wf_path)
            .with_context(|| format!("Failed to read workflow file: {display_name}"))?;
        let loaded_plan = plan_from_toml(&content)
            .with_context(|| format!("Failed to parse workflow file: {display_name}"))?;
        let jp = plan_journal_path(&project_root, &loaded_plan.name);
        (wf_path, loaded_plan, jp, false)
    };

    // 5. Parse --var KEY=VALUE into HashMap
    let cli_variables = parse_variables(&vars, &plan)?;

    // 6. Dry-run: print plan and exit
    if dry_run {
        print_plan(&plan, &cli_variables, config.as_ref());
        return Ok(());
    }

    let current_repo_fingerprint = detect_repo_fingerprint(&project_root);
    let resume_context = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &cli_variables,
        &current_repo_fingerprint,
        explicit_resume,
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
        workflow_path: &workflow_path,
        config: config.as_ref(),
        tool_override: tool_override.as_ref(),
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &resume_context.completed_steps,
        chunked,
    };

    let results =
        execute_plan_with_journal(&plan, &resume_context.initial_vars, &mut run_ctx).await?;

    // In chunked mode, stdout must be clean JSON only — skip summary and
    // final status updates (journal was already saved after the single step).
    if chunked {
        // Propagate step failure as process exit code.
        if let Some(r) = results.last()
            && r.exit_code != 0
            && !r.skipped
        {
            bail!(
                "Step {} ('{}') failed with exit code {}",
                r.step_id,
                r.title,
                r.exit_code
            );
        }
        return Ok(());
    }

    // 8. Print summary
    print_summary(&results, total_start.elapsed().as_secs_f64());

    if journal.status == "manual-handoff" {
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        eprintln!(
            "Workflow '{}' paused for manual handoff. Complete the requested main-agent action, then resume with `csa plan run --sa-mode true --resume {}`.",
            plan.name,
            journal_path.display()
        );
        return Ok(());
    }

    if journal.status == "awaiting-user" {
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        eprintln!(
            "Workflow '{}' is awaiting user action. Re-run the workflow from the beginning after the requested remediation is complete.",
            plan.name
        );
        return Ok(());
    }

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
            "{total_failures} step(s) failed ({execution_failures} execution, {unsupported_skips} unsupported-skip)"
        ));
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        bail!(
            "{total_failures} step(s) failed ({execution_failures} execution, {unsupported_skips} unsupported-skip)"
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
            .with_context(|| format!("Invalid --var format '{entry}': expected KEY=VALUE"))?;
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
        bail!("Invalid variable name '{name}': must match [A-Za-z_][A-Za-z0-9_]*");
    }

    if chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        bail!("Invalid variable name '{name}': must match [A-Za-z_][A-Za-z0-9_]*");
    }
}

/// Substitute `${VAR}` placeholders in a string (used by CSA steps only).
fn substitute_vars(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("${{{key}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

#[cfg(test)]
#[path = "plan_cmd_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "plan_cmd_tests_tail.rs"]
mod tests_tail;

#[cfg(test)]
#[path = "plan_cmd_tests_workflows.rs"]
mod tests_workflows;

#[cfg(test)]
#[path = "plan_cmd_tests_pr_bot.rs"]
mod tests_pr_bot;

#[cfg(test)]
#[path = "plan_cmd_tests_chunked.rs"]
mod tests_chunked;

#[cfg(test)]
#[path = "plan_cmd_tests_commit.rs"]
mod tests_commit;

#[cfg(test)]
#[path = "plan_cmd_override_tests.rs"]
mod override_tests;
