//! Batch task execution for parallel orchestration.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::pipeline::{ConfigRefs, determine_project_root, execute_with_session};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_resource::{ResourceGuard, ResourceLimits};

#[path = "batch_catalog.rs"]
mod batch_catalog;
use batch_catalog::register_batch_model_specs;

include!("batch_types.rs");
include!("batch_resource.rs");

/// Handle the batch command.
pub(crate) async fn handle_batch(
    file: String,
    cd: Option<String>,
    dry_run: bool,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Load one immutable model-sensitive snapshot for the whole command.
    let csa_config::EffectiveConfig {
        project: config,
        global: global_config,
        mut model_catalog,
        ..
    } = csa_config::EffectiveConfig::load(&project_root)?;

    // 3. Check recursion depth
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}",
            max_depth, current_depth
        );
        anyhow::bail!("Max recursion depth exceeded");
    }

    // 4. Load and parse batch TOML file
    let batch_path = PathBuf::from(&file);
    if !batch_path.exists() {
        anyhow::bail!("Batch file not found: {file}");
    }

    let batch_content = std::fs::read_to_string(&batch_path)
        .with_context(|| format!("Failed to read batch file: {file}"))?;

    let batch_config: BatchConfig = toml::from_str(&batch_content)
        .with_context(|| format!("Failed to parse batch file: {file}"))?;

    if batch_config.tasks.is_empty() {
        warn!("No tasks found in batch file");
        return Ok(());
    }

    // 5. Validate tasks
    validate_tasks(&batch_config.tasks)?;
    register_batch_model_specs(
        &mut model_catalog,
        &batch_config.tasks,
        &batch_path,
        config.as_ref(),
        &global_config,
        &project_root,
    )?;

    // 6. Build execution plan
    let execution_plan = build_execution_plan(&batch_config.tasks)?;

    // 7. If dry-run, print plan and exit
    if dry_run {
        print_execution_plan(&execution_plan, &batch_config.tasks);
        return Ok(());
    }

    // 8. Execute tasks
    info!(
        "Executing {} tasks from batch file",
        batch_config.tasks.len()
    );
    let config = config.map(Arc::new);
    let global_config = Arc::new(global_config);
    let model_catalog = Arc::new(model_catalog);
    let resource_overrides = RunResourceOverrides::new(None, None);
    let batch_context = BatchExecutionContext {
        project_root: &project_root,
        config,
        global_config,
        model_catalog,
        resource_overrides,
        startup_env,
    };
    let results = execute_batch(&execution_plan, &batch_config.tasks, &batch_context).await?;

    // 9. Print summary
    print_summary(&results);

    // 10. Exit with non-zero if any task failed
    let failed_count = results.iter().filter(|r| r.exit_code != 0).count();
    if failed_count > 0 {
        anyhow::bail!("{failed_count} tasks failed");
    }

    Ok(())
}

/// Validate batch tasks: check for duplicates, missing dependencies, cycles.
fn validate_tasks(tasks: &[BatchTask]) -> Result<()> {
    let mut names = HashSet::new();

    // Check for duplicate names
    for task in tasks {
        if !names.insert(&task.name) {
            anyhow::bail!("Duplicate task name: {}", task.name);
        }
    }

    // Check for missing dependencies
    for task in tasks {
        for dep in &task.depends_on {
            if !names.contains(dep) {
                anyhow::bail!("Task '{}' depends on unknown task '{}'", task.name, dep);
            }
        }
    }

    // Check for dependency cycles using DFS
    let task_map: HashMap<&str, &BatchTask> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    for task in tasks {
        detect_cycle(&task.name, &task_map, &mut HashSet::new(), &mut Vec::new())?;
    }

    Ok(())
}

/// Detect dependency cycles using DFS.
fn detect_cycle(
    current: &str,
    task_map: &HashMap<&str, &BatchTask>,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Result<()> {
    if path.contains(&current.to_string()) {
        anyhow::bail!(
            "Dependency cycle detected: {} -> {}",
            path.join(" -> "),
            current
        );
    }

    if visited.contains(current) {
        return Ok(());
    }

    visited.insert(current.to_string());
    path.push(current.to_string());

    if let Some(task) = task_map.get(current) {
        for dep in &task.depends_on {
            detect_cycle(dep, task_map, visited, path)?;
        }
    }

    path.pop();
    Ok(())
}

/// Execution plan: groups of tasks at each dependency level.
type ExecutionPlan = Vec<Vec<String>>;

/// Build execution plan by grouping tasks into dependency levels.
fn build_execution_plan(tasks: &[BatchTask]) -> Result<ExecutionPlan> {
    let mut plan = Vec::new();
    let mut completed = HashSet::new();

    // Keep scheduling tasks until all are complete
    while completed.len() < tasks.len() {
        let mut current_level = Vec::new();

        // Find tasks whose dependencies are all completed
        for task in tasks {
            if completed.contains(&task.name) {
                continue;
            }

            let deps_satisfied = task
                .depends_on
                .iter()
                .all(|dep| completed.contains(dep.as_str()));

            if deps_satisfied {
                current_level.push(task.name.clone());
            }
        }

        if current_level.is_empty() {
            anyhow::bail!("Unable to schedule remaining tasks (possible cycle)");
        }

        // Mark this level as completed
        for name in &current_level {
            completed.insert(name.clone());
        }

        plan.push(current_level);
    }

    Ok(plan)
}

/// Print the execution plan (for dry-run).
fn print_execution_plan(plan: &ExecutionPlan, tasks: &[BatchTask]) {
    println!("Execution Plan:");
    println!();

    for (level_idx, level) in plan.iter().enumerate() {
        println!("Level {}:", level_idx + 1);

        for task_name in level {
            // Find the task by name
            if let Some(task) = tasks.iter().find(|t| t.name == *task_name) {
                let mode_label = if task.mode == TaskMode::Parallel {
                    "(parallel)"
                } else {
                    "(sequential)"
                };

                println!(
                    "  - {} [{}] {} {}",
                    task.name,
                    task.tool,
                    mode_label,
                    if task.depends_on.is_empty() {
                        String::new()
                    } else {
                        format!("depends_on: {}", task.depends_on.join(", "))
                    }
                );
            }
        }

        println!();
    }

    println!("Total tasks: {}", tasks.len());
    println!("Total levels: {}", plan.len());
}

/// Execute the batch according to the execution plan.
async fn execute_batch(
    plan: &ExecutionPlan,
    tasks: &[BatchTask],
    context: &BatchExecutionContext<'_>,
) -> Result<Vec<TaskResult>> {
    let mut results = Vec::new();
    let task_map: HashMap<&str, &BatchTask> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    let limits = ResourceLimits {
        min_free_memory_mb: context
            .resource_overrides
            .resolve_min_free_memory_mb(context.config.as_deref()),
    };
    let mut resource_guard = Some(ResourceGuard::new(limits));

    // Execute each level
    for (level_idx, level) in plan.iter().enumerate() {
        info!(
            "Executing level {}/{} ({} tasks)",
            level_idx + 1,
            plan.len(),
            level.len()
        );

        check_level_resource_availability(level, &task_map, &mut resource_guard).with_context(
            || {
                format!(
                    "Resource check failed before spawning batch level {}",
                    level_idx + 1
                )
            },
        )?;

        // Separate parallel and sequential tasks in this level
        let mut parallel_tasks = Vec::new();
        let mut sequential_tasks = Vec::new();

        for task_name in level {
            if let Some(task) = task_map.get(task_name.as_str()) {
                if task.mode == TaskMode::Parallel && level.len() > 1 {
                    parallel_tasks.push((*task).clone());
                } else {
                    sequential_tasks.push((*task).clone());
                }
            }
        }

        // Execute parallel tasks concurrently
        if !parallel_tasks.is_empty() {
            let parallel_results =
                execute_parallel_tasks(&parallel_tasks, context, level_idx + 1).await?;
            results.extend(parallel_results);
        }

        // Execute sequential tasks one by one
        for (seq_idx, task) in sequential_tasks.iter().enumerate() {
            let result = execute_task(
                task,
                BatchTaskExecutionContext {
                    project_root: context.project_root,
                    config: context.config.as_deref(),
                    global_config: &context.global_config,
                    model_catalog: &context.model_catalog,
                    resource_guard: &mut resource_guard,
                    resource_overrides: context.resource_overrides,
                    level: level_idx + 1,
                    seq: seq_idx + 1,
                    startup_env: context.startup_env,
                },
            )
            .await;
            results.push(result);
        }
    }

    Ok(results)
}

/// Execute parallel tasks concurrently using JoinSet.
async fn execute_parallel_tasks(
    tasks: &[BatchTask],
    context: &BatchExecutionContext<'_>,
    level: usize,
) -> Result<Vec<TaskResult>> {
    let mut join_set = JoinSet::new();
    let project_root = context.project_root.to_path_buf();
    let startup_env = context.startup_env.clone();

    for task in tasks {
        let task = task.clone();
        let project_root = project_root.clone();
        let config = context.config.clone();
        let global_config = Arc::clone(&context.global_config);
        let model_catalog = Arc::clone(&context.model_catalog);
        let resource_overrides = context.resource_overrides;
        let startup_env = startup_env.clone();

        join_set.spawn(async move {
            execute_task(
                &task,
                BatchTaskExecutionContext {
                    project_root: &project_root,
                    config: config.as_deref(),
                    global_config: &global_config,
                    model_catalog: &model_catalog,
                    resource_guard: &mut None, // Parallel tasks avoid shared guard contention.
                    resource_overrides,
                    level,
                    seq: 0,
                    startup_env: &startup_env,
                },
            )
            .await
        });
    }

    let mut results = Vec::new();

    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(task_result) => results.push(task_result),
            Err(e) => {
                error!("Task join error: {}", e);
            }
        }
    }

    Ok(results)
}

/// Execute a single task.
async fn execute_task(task: &BatchTask, context: BatchTaskExecutionContext<'_>) -> TaskResult {
    let BatchTaskExecutionContext {
        project_root,
        config,
        global_config,
        model_catalog,
        resource_guard,
        resource_overrides,
        level,
        seq,
        startup_env,
    } = context;
    let start = Instant::now();
    let task_label = if seq > 0 {
        format!("[{}/{}] {}", level, seq, task.name)
    } else {
        format!("[{}] {}", level, task.name)
    };

    info!("{} - Starting ({}) ...", task_label, task.tool);

    // Parse tool name
    let tool_name = match parse_tool_name(&task.tool) {
        Ok(t) => t,
        Err(e) => {
            error!("{} - Failed to parse tool name: {}", task_label, e);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Invalid tool name: {e}")),
            };
        }
    };
    let resolved_model = batch_catalog::resolve_batch_model(task, config);

    // Check tool is enabled
    if let Some(cfg) = config {
        if !cfg.is_tool_enabled(tool_name.as_str()) {
            error!("{} - Tool disabled in config", task_label);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some("Tool disabled in config".to_string()),
            };
        }

        // Enforce tier whitelist: tool + model name
        if let Err(e) = cfg.enforce_tier_whitelist(tool_name.as_str(), None) {
            error!("{} - {}", task_label, e);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("{e}")),
            };
        }
        if let Err(e) = cfg.enforce_tier_model_name(
            tool_name.as_str(),
            crate::run_helpers::model_name_for_tier_validation(resolved_model.as_deref()),
        ) {
            error!("{} - {}", task_label, e);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("{e}")),
            };
        }
    }

    // Build and admit the final executor before dispatch.
    let executor = match crate::pipeline::build_and_validate_executor(
        &tool_name,
        None,
        resolved_model.as_deref(),
        None,
        ConfigRefs {
            project: config,
            global: Some(global_config),
            model_catalog: Some(model_catalog),
        },
        false,
        false,
        false,
    )
    .await
    {
        Ok(executor) => executor,
        Err(error) => {
            error!("{} - Failed to build executor: {}", task_label, error);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Failed to build executor: {error}")),
            };
        }
    };

    // Check resource availability
    if let Some(guard) = resource_guard
        && let Err(e) = guard.check_availability(executor.tool_name())
    {
        error!("{} - Resource check failed: {}", task_label, e);
        return TaskResult {
            name: task.name.clone(),
            exit_code: 1,
            duration_secs: start.elapsed().as_secs_f64(),
            error: Some(format!("Resource check failed: {e}")),
        };
    }

    let extra_env = global_config.build_execution_env(
        executor.tool_name(),
        csa_config::ExecutionEnvOptions::default(),
    );
    // #1741: a batch task picks its own tool/model from the batch TOML and does
    // NOT consume the parent's subtree pin for that choice, but it MUST still
    // cascade an inherited pin so nested CSA calls from the task stay pinned all
    // the way down. The pin is carried out-of-band as a typed value (None unless
    // this process is a pinned child) and applied by the executor's trusted
    // channel — never via the env map.
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    let subtree_pin =
        crate::run_cmd_model_pin::inherited_subtree_model_pin(inherited_model_pin.as_ref());
    let extra_env_ref = extra_env.as_ref();
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config, None);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_for_tool(
            config,
            None,
            None,
            executor.tool_name(),
        );

    // Acquire global slot to enforce concurrency limit (fail-fast)
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = match csa_config::GlobalConfig::slots_dir() {
        Ok(d) => d,
        Err(e) => {
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Failed to resolve slots directory: {e}")),
            };
        }
    };
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => slot,
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!(
                    "All {} slots for '{}' occupied ({}/{})",
                    max_concurrent,
                    executor.tool_name(),
                    status.occupied,
                    status.max_slots,
                )),
            };
        }
        Err(e) => {
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!(
                    "Slot acquisition failed for '{}': {}",
                    executor.tool_name(),
                    e
                )),
            };
        }
    };

    // Execute with ephemeral session (no persistent state)
    let result = execute_with_session(
        &executor,
        &tool_name,
        &task.prompt,
        None,                                            // session_arg: None (ephemeral)
        false,                                           // fresh_spawn_preflight_override
        Some(format!("batch: {}", task.name)),           // description
        startup_env.session_id().map(ToOwned::to_owned), // parent
        project_root,
        config,
        extra_env_ref,
        subtree_pin.as_ref(),
        Some("batch"),
        None, // batch does not use tier-based selection
        None, // batch does not override context loading options
        csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None, // batch does not set wall-clock timeout
        None, // batch does not use memory injection
        None, // batch does not inject MCP (callers don't have global_config)
        None, // batch does not run pre-session hooks
        resource_overrides.for_child(),
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
        None, // error_marker_scan_override: batch has no CLI flag; defer to marker/config (#1745/#1847)
        false, // cli_no_hook_bypass_scan: no CLI flag here; defer to config
        startup_env,
    )
    .await;

    let duration = start.elapsed().as_secs_f64();

    match result {
        Ok(exec_result) => {
            if exec_result.exit_code == 0 {
                info!(
                    "{} - Completed successfully in {:.2}s",
                    task_label, duration
                );
            } else {
                error!(
                    "{} - Failed with exit code {} in {:.2}s",
                    task_label, exec_result.exit_code, duration
                );
            }

            TaskResult {
                name: task.name.clone(),
                exit_code: exec_result.exit_code,
                duration_secs: duration,
                error: None,
            }
        }
        Err(e) => {
            error!("{} - Execution error: {}", task_label, e);
            TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: duration,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Parse tool name string to ToolName enum.
fn parse_tool_name(tool: &str) -> Result<ToolName> {
    match tool {
        "gemini-cli" | "gemini" => {
            anyhow::bail!("{}", csa_core::types::removed_tool_error("gemini-cli"))
        }
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        _ => anyhow::bail!("Unknown tool: {tool}"),
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;

/// Print execution summary.
fn print_summary(results: &[TaskResult]) {
    println!();
    println!("=== Batch Execution Summary ===");
    println!();

    let mut success_count = 0;
    let mut failed_count = 0;
    let total_duration: f64 = results.iter().map(|r| r.duration_secs).sum();

    for result in results {
        let status = if result.exit_code == 0 {
            success_count += 1;
            "✓ PASS"
        } else {
            failed_count += 1;
            "✗ FAIL"
        };

        println!(
            "{:8} {} ({:.2}s){}",
            status,
            result.name,
            result.duration_secs,
            if let Some(ref err) = result.error {
                format!(" - {err}")
            } else {
                String::new()
            }
        );
    }

    println!();
    println!("Total: {} tasks", results.len());
    println!("Success: {success_count}");
    println!("Failed: {failed_count}");
    println!("Total duration: {total_duration:.2}s");
}
