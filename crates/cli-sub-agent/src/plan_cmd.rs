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
//! - `${VAR}` substitution for CSA prompts, step tiers, and condition evaluation
//! - `on_fail` handling: abort / skip / retry N
//! - `condition` evaluation: `${VAR}` truthiness, `!(expr)`, `(a) && (b)`
//! - Steps with `loop_var` are skipped with a warning (v2)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use weave::compiler::{ExecutionPlan, plan_from_toml};

use crate::pattern_resolver;
use crate::pipeline::determine_project_root;
use crate::plan_display::{print_plan, print_summary};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

#[path = "plan_cmd_exec.rs"]
mod plan_cmd_exec;
#[cfg(test)]
use plan_cmd_exec::{extract_bash_code_block, truncate};

#[path = "plan_cmd_flow.rs"]
mod plan_cmd_flow;

#[path = "plan_cmd_repo.rs"]
mod plan_cmd_repo;

#[path = "plan_cmd_failure.rs"]
pub(crate) mod plan_cmd_failure;

#[path = "plan_cmd_completion.rs"]
pub(crate) mod plan_cmd_completion;

#[path = "plan_cmd_assignment.rs"]
mod plan_cmd_assignment;

#[path = "plan_cmd_tier_failover.rs"]
mod plan_cmd_tier_failover;

#[path = "plan_cmd_child_diagnostics.rs"]
mod plan_cmd_child_diagnostics;
#[path = "plan_cmd_steps.rs"]
mod plan_cmd_steps;
#[cfg(test)]
#[path = "plan_cmd_steps_test_helpers.rs"]
mod plan_cmd_steps_test_helpers;
#[cfg(test)]
pub(crate) use plan_cmd_assignment::{
    extract_output_assignment_markers, is_assignment_marker_key, should_inject_assignment_markers,
};
pub(crate) use plan_cmd_flow::shell_escape_for_command;
pub(crate) use plan_cmd_repo::detect_effective_repo;
#[cfg(test)]
pub(crate) use plan_cmd_steps::resolve_step_tool;
use plan_cmd_steps::{PlanRunContext, execute_plan_with_journal};
pub(crate) use plan_cmd_steps::{StepResult, StepTarget, resolve_step_tool_with_variables};
#[cfg(test)]
pub(crate) use plan_cmd_steps_test_helpers::{
    execute_plan, execute_step, test_global_config, test_model_catalog,
};

// Journal, resume-context, and repo-fingerprint primitives live in
// `plan_cmd_journal` to keep this module within the per-file token budget.
// Re-exported here so the daemon dispatch (`crate::plan_cmd::PlanRunPipelineSource`)
// and the in-module step/test submodules (`super::*`) keep their original paths.
pub(crate) use crate::plan_cmd_journal::{
    PLAN_JOURNAL_SCHEMA_VERSION, PlanRunJournal, PlanRunPipelineSource, apply_repo_fingerprint,
    complete_pending_manual_step, detect_repo_fingerprint, load_plan_resume_context,
    persist_plan_journal, plan_journal_path,
};
// Referenced only from the `#[cfg(test)]` submodules; gated to avoid an
// unused-import error in non-test builds.
#[cfg(test)]
pub(crate) use crate::plan_cmd_journal::{
    PLAN_PIPELINE_SOURCE_CLI_ALIAS, PLAN_PIPELINE_SOURCE_DIRECT, default_plan_pipeline_source,
    normalize_path, safe_plan_name,
};

/// Workflow variable containing the fetched issue body from
/// `csa plan run --issue <N>`.
pub(crate) const FEATURE_INPUT_VAR: &str = "FEATURE_INPUT";

/// Workflow variable containing the numeric issue number from
/// `csa plan run --issue <N>`.
pub(crate) const ISSUE_NUMBER_VAR: &str = "ISSUE_NUMBER";

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
    pub model_spec_override: Option<String>,
    pub dry_run: bool,
    pub chunked: bool,
    pub resume: Option<String>,
    pub complete_manual_step: Option<usize>,
    pub cd: Option<String>,
    pub no_fs_sandbox: bool,
    pub resources: RunResourceOverrides,
    pub current_depth: u32,
    pub pipeline_source: PlanRunPipelineSource,
    pub startup_env: StartupSubtreeEnv,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PlanRunOutcome {
    pub(crate) completion_summary: Option<String>,
}

/// Handle `csa plan run <file>` or `csa plan run --pattern <name>`.
///
/// Foreground entry point: runs the workflow synchronously in the calling
/// process. Used by `--foreground` callers, `--dry-run`, `--chunked`,
/// `--resume`, the daemon-child path (after env priming), and any nested
/// invocation detected by [`plan_cmd_daemon::dispatch`] (depth>0 / parent
/// session env), which depend on the synchronous exit-code contract.
pub(crate) async fn handle_plan_run(args: PlanRunArgs) -> Result<PlanRunOutcome> {
    let PlanRunArgs {
        file,
        pattern,
        vars,
        tool_override,
        model_spec_override,
        dry_run,
        chunked,
        resume,
        complete_manual_step,
        cd,
        no_fs_sandbox,
        resources,
        current_depth,
        pipeline_source,
        startup_env,
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

    // 2. Load one immutable model-sensitive snapshot for the whole command.
    let csa_config::EffectiveConfig {
        project: config,
        global: global_config,
        model_catalog,
        ..
    } = csa_config::EffectiveConfig::load(&project_root)?;

    // 3. Check recursion depth
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);
    if current_depth > max_depth {
        bail!("Max recursion depth ({max_depth}) exceeded. Current: {current_depth}");
    }
    enforce_plan_run_tier_bypass_gate(
        config.as_ref(),
        &global_config,
        model_spec_override.as_deref(),
        &startup_env,
    )?;

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
        return Ok(PlanRunOutcome::default());
    }

    if let Some(step_id) = complete_manual_step {
        if !explicit_resume {
            bail!("--complete-manual-step requires --resume");
        }
        complete_pending_manual_step(&plan, &workflow_path, &journal_path, step_id)?;
        eprintln!(
            "csa plan: marked manual step {step_id} complete in journal {}",
            journal_path.display()
        );
    }

    let completion_snapshot =
        plan_cmd_completion::PlanCompletionSnapshot::capture(&plan.name, &project_root);
    let current_repo_fingerprint = detect_repo_fingerprint(&project_root);
    let resume_context = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &cli_variables,
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
    let recovery_snapshot =
        plan_cmd_failure::capture_failure_recovery_snapshot(&plan.name, &project_root);
    let total_start = Instant::now();
    if let Some(ref tool) = tool_override {
        eprintln!("  Tool override: --tool {} (all CSA steps)", tool.as_str());
    }
    let mut journal = PlanRunJournal::new(
        &plan.name,
        &workflow_path,
        resume_context.initial_vars.clone(),
    );
    journal.pipeline_source = resume_context
        .pipeline_source
        .clone()
        .unwrap_or_else(|| pipeline_source.as_str().to_string());
    journal.completed_steps = resume_context.completed_steps.iter().copied().collect();
    apply_repo_fingerprint(&mut journal, &current_repo_fingerprint);
    persist_plan_journal(&journal_path, &journal)?;
    let mut run_ctx = PlanRunContext {
        project_root: &project_root,
        workflow_path: &workflow_path,
        config: config.as_ref(),
        global_config: &global_config,
        model_catalog: &model_catalog,
        tool_override: tool_override.as_ref(),
        model_spec_override: model_spec_override.as_ref(),
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &resume_context.completed_steps,
        chunked,
        no_fs_sandbox,
        resources,
        startup_env: &startup_env,
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
        return Ok(PlanRunOutcome::default());
    }

    // 8. Print summary
    print_summary(&results, total_start.elapsed().as_secs_f64());

    if journal.status == "manual-handoff" {
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        let completed_steps: std::collections::HashSet<usize> =
            journal.completed_steps.iter().copied().collect();
        let resume_cmd = plan
            .steps
            .iter()
            .find(|step| !completed_steps.contains(&step.id))
            .map(|step| {
                plan_cmd_flow::format_manual_step_resume_command(
                    &project_root,
                    &workflow_path,
                    Some(&journal_path),
                    step.id,
                )
            })
            .unwrap_or_else(|| {
                plan_cmd_flow::format_plan_resume_command(
                    &project_root,
                    &workflow_path,
                    Some(&journal_path),
                )
            });
        eprintln!(
            "Workflow '{}' paused for manual handoff. Complete the requested main-agent action, then resume with `{}`.",
            plan.name, resume_cmd
        );
        return Ok(PlanRunOutcome {
            completion_summary: Some(format!(
                "workflow paused for manual handoff; next_action={resume_cmd}"
            )),
        });
    }

    if journal.status == "awaiting-user" {
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        eprintln!(
            "Workflow '{}' is awaiting user action. Re-run the workflow from the beginning after the requested remediation is complete.",
            plan.name
        );
        return Ok(PlanRunOutcome {
            completion_summary: Some(
                "workflow awaiting user action; next_action=rerun after requested remediation"
                    .to_string(),
            ),
        });
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
        let failure_summary = format!(
            "{total_failures} step(s) failed ({execution_failures} execution, {unsupported_skips} unsupported-skip)"
        );
        let recovery = recovery_snapshot
            .as_ref()
            .map(|snapshot| snapshot.recover_after_failure(&project_root));
        let verified_failure = plan_cmd_completion::verify_pr_bot_failure_side_effects(
            plan_cmd_completion::PrBotFailureSideEffectInput {
                workflow_name: &plan.name,
                workflow_path: &workflow_path,
                project_root: &project_root,
                results: &results,
                completed_steps: &journal.completed_steps,
                vars: &journal.vars,
                failure_summary: &failure_summary,
            },
        );
        let failure_error = verified_failure.unwrap_or_else(|| {
            let failure_report = plan_cmd_failure::PlanFailureReport::from_results(
                &plan.name,
                &workflow_path,
                failure_summary.clone(),
                &results,
                recovery,
            );
            plan_cmd_failure::PlanFailureError::new(failure_summary.clone(), failure_report)
        });
        let failure_summary = failure_error.to_string();
        journal.status = "failed".to_string();
        journal.last_error = Some(failure_summary.clone());
        apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
        persist_plan_journal(&journal_path, &journal)?;
        return Err(failure_error.into());
    }

    let completion_summary = match plan_cmd_completion::verify_plan_completion(
        plan_cmd_completion::PlanCompletionInput {
            workflow_name: &plan.name,
            workflow_path: &workflow_path,
            project_root: &project_root,
            results: &results,
            completed_steps: &journal.completed_steps,
            vars: &journal.vars,
            snapshot: &completion_snapshot,
        },
    ) {
        Ok(summary) => summary,
        Err(err) => {
            let failure_summary = err.to_string();
            journal.status = "failed".to_string();
            journal.last_error = Some(failure_summary.clone());
            apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
            persist_plan_journal(&journal_path, &journal)?;
            return Err(err.into());
        }
    };

    journal.status = "completed".to_string();
    journal.last_error = None;
    apply_repo_fingerprint(&mut journal, &detect_repo_fingerprint(&project_root));
    persist_plan_journal(&journal_path, &journal)?;

    Ok(PlanRunOutcome { completion_summary })
}

fn enforce_plan_run_tier_bypass_gate(
    config: Option<&ProjectConfig>,
    global_config: &csa_config::GlobalConfig,
    model_spec_override: Option<&str>,
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
    let Some(model_spec) = model_spec_override else {
        return Ok(());
    };
    let inherited_trusted_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env)
            .is_some_and(|pin| pin.force_ignore_tier_setting && pin.model_spec == model_spec);
    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config: config,
        global_config,
        flags: crate::run_helpers::TierBypassGateFlags {
            model_spec: true,
            force: false,
            force_ignore_tier_setting: false,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin,
    })
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

/// Substitute `${VAR}` placeholders in a workflow string.
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
#[path = "plan_cmd_tests_issue.rs"]
mod tests_issue;

#[cfg(test)]
#[path = "plan_cmd_tests_manual_handoff.rs"]
mod tests_manual_handoff;

#[cfg(test)]
#[path = "plan_cmd_tests_workflows.rs"]
mod tests_workflows;

#[cfg(test)]
#[path = "plan_cmd_tests_mktd_save.rs"]
mod tests_mktd_save;

#[cfg(test)]
#[path = "plan_cmd_tests_mktd_save_schema.rs"]
mod tests_mktd_save_schema;

#[cfg(test)]
#[path = "plan_cmd_tests_mktd_save_workflow.rs"]
mod tests_mktd_save_workflow;

#[cfg(test)]
#[path = "plan_cmd_tests_dev2merge_2031.rs"]
mod tests_dev2merge_2031;

#[cfg(test)]
#[path = "plan_cmd_tests_dev2merge_2305.rs"]
mod tests_dev2merge_2305;

#[cfg(test)]
#[path = "plan_cmd_tests_pr_bot.rs"]
mod tests_pr_bot;

#[cfg(test)]
#[path = "plan_cmd_tests_pr_bot_degraded.rs"]
mod tests_pr_bot_degraded;

#[cfg(test)]
#[path = "plan_cmd_tests_pr_bot_2014.rs"]
mod tests_pr_bot_2014;

#[cfg(test)]
#[path = "plan_cmd_tests_chunked.rs"]
mod tests_chunked;

#[cfg(test)]
#[path = "plan_cmd_tests_commit.rs"]
mod tests_commit;

#[cfg(test)]
#[path = "plan_cmd_override_tests.rs"]
mod override_tests;
