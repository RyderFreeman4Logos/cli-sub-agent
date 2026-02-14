//! Batch task execution for parallel orchestration.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_process::check_tool_installed;
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::get_session_root;

use crate::pipeline::{determine_project_root, execute_with_session};
use crate::run_helpers::build_executor;

/// Batch configuration loaded from TOML file.
#[derive(Debug, Deserialize)]
struct BatchConfig {
    tasks: Vec<BatchTask>,
}

/// A single task in the batch.
#[derive(Debug, Clone, Deserialize)]
struct BatchTask {
    /// Task name (unique identifier)
    name: String,

    /// Tool to use (gemini-cli, opencode, codex, claude-code)
    tool: String,

    /// Task prompt
    prompt: String,

    /// Execution mode: sequential (default) or parallel
    #[serde(default)]
    mode: TaskMode,

    /// Task dependencies (must complete before this task starts)
    #[serde(default)]
    depends_on: Vec<String>,

    /// Optional model override
    #[serde(default)]
    model: Option<String>,
}

/// Task execution mode.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum TaskMode {
    #[default]
    Sequential,
    Parallel,
}

/// Task execution result.
#[derive(Debug)]
struct TaskResult {
    name: String,
    exit_code: i32,
    duration_secs: f64,
    error: Option<String>,
}

/// Handle the batch command.
pub(crate) async fn handle_batch(
    file: String,
    cd: Option<String>,
    dry_run: bool,
    current_depth: u32,
) -> Result<()> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

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
        anyhow::bail!("Batch file not found: {}", file);
    }

    let batch_content = std::fs::read_to_string(&batch_path)
        .with_context(|| format!("Failed to read batch file: {}", file))?;

    let batch_config: BatchConfig = toml::from_str(&batch_content)
        .with_context(|| format!("Failed to parse batch file: {}", file))?;

    if batch_config.tasks.is_empty() {
        warn!("No tasks found in batch file");
        return Ok(());
    }

    // 5. Validate tasks
    validate_tasks(&batch_config.tasks)?;

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
    let results = execute_batch(
        &execution_plan,
        &batch_config.tasks,
        &project_root,
        config.as_ref(),
    )
    .await?;

    // 9. Print summary
    print_summary(&results);

    // 10. Exit with non-zero if any task failed
    let failed_count = results.iter().filter(|r| r.exit_code != 0).count();
    if failed_count > 0 {
        anyhow::bail!("{} tasks failed", failed_count);
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
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<Vec<TaskResult>> {
    let mut results = Vec::new();
    let task_map: HashMap<&str, &BatchTask> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    // Create resource guard if config exists
    let mut resource_guard = if let Some(cfg) = config {
        let limits = ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
            initial_estimates: cfg.resources.initial_estimates.clone(),
        };
        let project_state_dir = get_session_root(project_root)?;
        let stats_path = project_state_dir.join("usage_stats.toml");
        Some(ResourceGuard::new(limits, &stats_path))
    } else {
        None
    };

    // Execute each level
    for (level_idx, level) in plan.iter().enumerate() {
        info!(
            "Executing level {}/{} ({} tasks)",
            level_idx + 1,
            plan.len(),
            level.len()
        );

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
            let parallel_results = execute_parallel_tasks(
                &parallel_tasks,
                project_root,
                config,
                &mut resource_guard,
                level_idx + 1,
                tasks.len(),
            )
            .await?;
            results.extend(parallel_results);
        }

        // Execute sequential tasks one by one
        for (seq_idx, task) in sequential_tasks.iter().enumerate() {
            let result = execute_task(
                task,
                project_root,
                config,
                &mut resource_guard,
                level_idx + 1,
                seq_idx + 1,
                tasks.len(),
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
    project_root: &Path,
    config: Option<&ProjectConfig>,
    resource_guard: &mut Option<ResourceGuard>,
    level: usize,
    total_tasks: usize,
) -> Result<Vec<TaskResult>> {
    let mut join_set = JoinSet::new();
    let project_root = project_root.to_path_buf();

    for task in tasks {
        let task = task.clone();
        let project_root = project_root.clone();
        let config = config.cloned();

        // Check resource availability before spawning (best effort)
        if let Some(guard) = resource_guard {
            let tool_name = parse_tool_name(&task.tool)?;
            if let Err(e) = guard.check_availability(tool_name.as_str()) {
                warn!(
                    "Resource check failed for task '{}': {}. Proceeding anyway.",
                    task.name, e
                );
            }
        }

        join_set.spawn(async move {
            execute_task(
                &task,
                &project_root,
                config.as_ref(),
                &mut None, // No resource guard in parallel (to avoid contention)
                level,
                0,
                total_tasks,
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
async fn execute_task(
    task: &BatchTask,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    resource_guard: &mut Option<ResourceGuard>,
    level: usize,
    seq: usize,
    _total: usize,
) -> TaskResult {
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
                error: Some(format!("Invalid tool name: {}", e)),
            };
        }
    };

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
    }

    // Build executor
    let executor = match build_executor(&tool_name, None, task.model.as_deref(), None, config) {
        Ok(e) => e,
        Err(e) => {
            error!("{} - Failed to build executor: {}", task_label, e);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Failed to build executor: {}", e)),
            };
        }
    };

    // Check tool is installed (using runtime binary name for ACP-aware check)
    if let Err(e) = check_tool_installed(executor.runtime_binary_name()).await {
        error!("{} - Tool not installed: {}", task_label, e);
        return TaskResult {
            name: task.name.clone(),
            exit_code: 1,
            duration_secs: start.elapsed().as_secs_f64(),
            error: Some(format!("Tool not installed: {}", e)),
        };
    }

    // Check resource availability
    if let Some(guard) = resource_guard {
        if let Err(e) = guard.check_availability(executor.tool_name()) {
            error!("{} - Resource check failed: {}", task_label, e);
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Resource check failed: {}", e)),
            };
        }
    }

    // Load global config for env injection and slot control
    let global_config = match csa_config::GlobalConfig::load() {
        Ok(gc) => gc,
        Err(e) => {
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Failed to load global config: {}", e)),
            };
        }
    };
    let extra_env = global_config.env_vars(executor.tool_name()).cloned();
    let extra_env_ref = extra_env.as_ref();

    // Acquire global slot to enforce concurrency limit (fail-fast)
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = match csa_config::GlobalConfig::slots_dir() {
        Ok(d) => d,
        Err(e) => {
            return TaskResult {
                name: task.name.clone(),
                exit_code: 1,
                duration_secs: start.elapsed().as_secs_f64(),
                error: Some(format!("Failed to resolve slots directory: {}", e)),
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
        None,                                  // session_arg: None (ephemeral)
        Some(format!("batch: {}", task.name)), // description
        std::env::var("CSA_SESSION_ID").ok(),  // parent
        project_root,
        config,
        extra_env_ref,
        Some("batch"),
        None, // batch does not use tier-based selection
        csa_process::StreamMode::BufferOnly,
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
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        _ => anyhow::bail!("Unknown tool: {}", tool),
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
                format!(" - {}", err)
            } else {
                String::new()
            }
        );
    }

    println!();
    println!("Total: {} tasks", results.len());
    println!("Success: {}", success_count);
    println!("Failed: {}", failed_count);
    println!("Total duration: {:.2}s", total_duration);
}
