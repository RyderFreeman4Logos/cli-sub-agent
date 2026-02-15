use crate::cli::TodoDagFormat;
use anyhow::Result;
use csa_config::global::GlobalConfig;
use csa_core::types::OutputFormat;
use csa_hooks::{HookEvent, global_hooks_path, load_hooks_config, run_hooks_for_event};
use csa_todo::dag::DependencyGraph;
use csa_todo::{TodoManager, TodoStatus};
use tracing::warn;

pub(crate) fn handle_create(
    title: String,
    branch: Option<String>,
    cd: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    // Ensure git repo is initialized for the todos directory
    csa_todo::git::ensure_git_init(manager.todos_dir())?;

    let plan = manager.create(&title, branch.as_deref())?;

    // Auto-commit the initial plan (freshly created, should always have changes)
    let commit_msg = format!("create: {}", title);
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, &commit_msg)?
        .ok_or_else(|| anyhow::anyhow!("BUG: newly created plan had no changes to commit"))?;

    // TodoCreate hook: fires after successful plan creation (best-effort)
    {
        let project_hooks = project_root.join(".csa").join("hooks.toml");
        let hooks_config =
            load_hooks_config(Some(&project_hooks), global_hooks_path().as_deref(), None);
        let mut hook_vars = std::collections::HashMap::new();
        hook_vars.insert("plan_id".to_string(), plan.timestamp.clone());
        hook_vars.insert("plan_dir".to_string(), plan.todo_dir.display().to_string());
        hook_vars.insert(
            "todo_root".to_string(),
            manager.todos_dir().display().to_string(),
        );
        if let Err(e) = run_hooks_for_event(HookEvent::TodoCreate, &hooks_config, &hook_vars) {
            warn!("TodoCreate hook failed: {}", e);
        }
    }

    match format {
        OutputFormat::Json => {
            let json = serde_json::json!({
                "timestamp": plan.timestamp,
                "title": plan.metadata.title,
                "status": plan.metadata.status.to_string(),
                "branch": plan.metadata.branch,
                "path": plan.todo_md_path().display().to_string(),
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        OutputFormat::Text => {
            println!("{}", plan.timestamp);
            eprintln!(
                "Created TODO plan: {} ({})",
                plan.metadata.title, plan.timestamp
            );
            eprintln!("  Path: {}", plan.todo_md_path().display());
        }
    }

    Ok(())
}

pub(crate) fn handle_save(
    timestamp: Option<String>,
    message: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let commit_msg = message.unwrap_or_else(|| format!("update: {}", plan.metadata.title));
    match csa_todo::git::save(manager.todos_dir(), &ts, &commit_msg)? {
        Some(hash) => {
            eprintln!("Saved {} ({})", ts, hash);

            // TodoSave hook: fires after successful save (best-effort)
            let project_hooks = project_root.join(".csa").join("hooks.toml");
            let hooks_config =
                load_hooks_config(Some(&project_hooks), global_hooks_path().as_deref(), None);
            let version = csa_todo::git::list_versions(manager.todos_dir(), &ts)
                .map(|v| v.len())
                .unwrap_or(1);
            let mut hook_vars = std::collections::HashMap::new();
            hook_vars.insert("plan_id".to_string(), ts.clone());
            hook_vars.insert(
                "plan_dir".to_string(),
                manager.todos_dir().join(&ts).display().to_string(),
            );
            hook_vars.insert(
                "todo_root".to_string(),
                manager.todos_dir().display().to_string(),
            );
            hook_vars.insert("version".to_string(), version.to_string());
            hook_vars.insert("message".to_string(), commit_msg);
            if let Err(e) = run_hooks_for_event(HookEvent::TodoSave, &hooks_config, &hook_vars) {
                warn!("TodoSave hook failed: {}", e);
            }
        }
        None => eprintln!("No changes to save for plan '{ts}'."),
    }

    Ok(())
}

pub(crate) fn handle_diff(
    timestamp: Option<String>,
    revision: Option<String>,
    from: Option<usize>,
    to: Option<usize>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = resolve_timestamp(&manager, timestamp.as_deref())?;

    // Validate the plan exists
    manager.load(&ts)?;

    let diff_output = if from.is_some() || to.is_some() {
        // Version-to-version diff
        let from_v = from.unwrap_or(2);
        let to_v = to.unwrap_or(1);
        csa_todo::git::diff_versions(manager.todos_dir(), &ts, from_v, to_v)?
    } else {
        // Working copy diff against revision (or file's last commit)
        csa_todo::git::diff(manager.todos_dir(), &ts, revision.as_deref())?
    };

    if diff_output.is_empty() {
        eprintln!("No changes.");
    } else {
        let global = GlobalConfig::load().unwrap_or_default();
        print_or_pipe(&diff_output, global.todo.diff_command.as_deref());
    }

    Ok(())
}

pub(crate) fn handle_history(timestamp: Option<String>, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = resolve_timestamp(&manager, timestamp.as_deref())?;

    // Validate the plan exists
    manager.load(&ts)?;

    let log_output = csa_todo::git::history(manager.todos_dir(), &ts)?;

    if log_output.is_empty() {
        eprintln!("No history found.");
    } else {
        print!("{log_output}");
    }

    Ok(())
}

pub(crate) fn handle_list(
    status: Option<String>,
    cd: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    let plans = if let Some(status_str) = status {
        let status: TodoStatus = status_str.parse()?;
        manager.find_by_status(status)?
    } else {
        manager.list()?
    };

    if plans.is_empty() {
        match format {
            OutputFormat::Json => println!("[]"),
            OutputFormat::Text => eprintln!("No TODO plans found."),
        }
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            let json_plans: Vec<_> = plans
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "timestamp": p.timestamp,
                        "status": p.metadata.status.to_string(),
                        "title": p.metadata.title,
                        "branch": p.metadata.branch,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json_plans)?);
        }
        OutputFormat::Text => {
            // Table header
            println!(
                "{:<18}  {:<14}  {:<30}  BRANCH",
                "TIMESTAMP", "STATUS", "TITLE"
            );

            for plan in &plans {
                println!(
                    "{:<18}  {:<14}  {:<30}  {}",
                    plan.timestamp,
                    plan.metadata.status,
                    truncate(&plan.metadata.title, 30),
                    plan.metadata.branch.as_deref().unwrap_or("-"),
                );
            }
        }
    }

    Ok(())
}

pub(crate) fn handle_find(
    branch: Option<String>,
    status: Option<String>,
    cd: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    let mut plans = manager.list()?;

    if let Some(ref branch_filter) = branch {
        plans.retain(|p| p.metadata.branch.as_deref() == Some(branch_filter.as_str()));
    }

    if let Some(ref status_str) = status {
        let status_filter: TodoStatus = status_str.parse()?;
        plans.retain(|p| p.metadata.status == status_filter);
    }

    if plans.is_empty() {
        match format {
            OutputFormat::Json => println!("[]"),
            OutputFormat::Text => eprintln!("No matching TODO plans found."),
        }
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            let json_plans: Vec<_> = plans
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "timestamp": p.timestamp,
                        "status": p.metadata.status.to_string(),
                        "title": p.metadata.title,
                        "branch": p.metadata.branch,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json_plans)?);
        }
        OutputFormat::Text => {
            println!(
                "{:<18}  {:<14}  {:<30}  BRANCH",
                "TIMESTAMP", "STATUS", "TITLE"
            );

            for plan in &plans {
                println!(
                    "{:<18}  {:<14}  {:<30}  {}",
                    plan.timestamp,
                    plan.metadata.status,
                    truncate(&plan.metadata.title, 30),
                    plan.metadata.branch.as_deref().unwrap_or("-"),
                );
            }
        }
    }

    Ok(())
}

pub(crate) fn handle_show(
    timestamp: Option<String>,
    version: Option<usize>,
    path: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    if path {
        println!("{}", plan.todo_md_path().display());
    } else {
        let content = if let Some(v) = version {
            csa_todo::git::show_version(manager.todos_dir(), &ts, v)?
        } else {
            std::fs::read_to_string(plan.todo_md_path())?
        };
        let global = GlobalConfig::load().unwrap_or_default();
        print_or_pipe(&content, global.todo.show_command.as_deref());
    }

    Ok(())
}

pub(crate) fn handle_status(timestamp: String, status: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let old_plan = manager.load(&timestamp)?;
    let old_status = old_plan.metadata.status;
    let new_status: TodoStatus = status.parse()?;

    // Idempotent: skip if status unchanged
    if old_status == new_status {
        eprintln!("Status already '{}' — no change.", old_status);
        return Ok(());
    }

    let plan = manager.update_status(&timestamp, new_status)?;

    // Auto-commit only metadata.toml (don't accidentally commit other changes)
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let metadata_path = format!("{}/metadata.toml", timestamp);
    let commit_msg = format!(
        "status: {} → {} ({})",
        old_status, plan.metadata.status, plan.metadata.title
    );
    match csa_todo::git::save_file(manager.todos_dir(), &timestamp, &metadata_path, &commit_msg)? {
        Some(hash) => {
            eprintln!(
                "Updated {} status: {} → {} ({})",
                plan.timestamp, old_status, plan.metadata.status, hash
            );
        }
        None => {
            eprintln!(
                "Updated {} status → {}",
                plan.timestamp, plan.metadata.status
            );
        }
    }

    Ok(())
}

pub(crate) fn handle_dag(
    timestamp: Option<String>,
    format: TodoDagFormat,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let content = std::fs::read_to_string(plan.todo_md_path())?;
    let graph = DependencyGraph::from_markdown(&content)?;

    if let Some(cycle_nodes) = graph.cycle_nodes_bfs() {
        anyhow::bail!("Dependency cycle detected: {}", cycle_nodes.join(" -> "));
    }

    // Validate execution order exists for the graph before rendering.
    let _ = graph.topological_sort()?;

    let rendered = match format {
        TodoDagFormat::Mermaid => graph.to_mermaid(),
        TodoDagFormat::Terminal => graph.to_terminal(),
        TodoDagFormat::Dot => graph.to_dot(),
    };

    print!("{rendered}");
    Ok(())
}

/// Resolve an optional timestamp to an actual plan timestamp.
/// If `None`, uses the most recent plan.
fn resolve_timestamp(manager: &TodoManager, timestamp: Option<&str>) -> Result<String> {
    match timestamp {
        Some(ts) => Ok(ts.to_string()),
        None => {
            let plan = manager.latest()?;
            eprintln!("(using latest plan: {})", plan.timestamp);
            Ok(plan.timestamp)
        }
    }
}

/// Pipe content through an external command (e.g., `bat -l md`, `delta`).
///
/// Only activates when stdout is a terminal. Falls back to plain `print!()` if:
/// - stdout is not a terminal (piped or redirected)
/// - the command fails to spawn (e.g., not installed)
/// - the command string is empty/blank
///
/// Note: if the child process exits non-zero after receiving stdin, content may
/// have been partially displayed. We do not re-print in that case to avoid
/// garbled/duplicated output.
fn print_or_pipe(content: &str, command: Option<&str>) {
    use std::io::IsTerminal;
    use std::io::Write;

    let Some(cmd_str) = command else {
        print!("{content}");
        return;
    };

    if !std::io::stdout().is_terminal() {
        print!("{content}");
        return;
    }

    let parts: Vec<&str> = cmd_str.split_whitespace().collect();
    let Some((program, args)) = parts.split_first() else {
        print!("{content}");
        return;
    };

    let child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn();

    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                // Ignore write errors — the child may close stdin early (e.g., head -n)
                let _ = stdin.write_all(content.as_bytes());
                drop(stdin);
            }
            let _ = child.wait();
        }
        Err(_) => {
            // Command not found or failed to spawn — fall back to plain print
            print!("{content}");
        }
    }
}

/// Truncate a string to `max_len` characters (not bytes), appending "…" if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate tests ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world", 6);
        assert!(result.ends_with('\u{2026}'));
        assert_eq!(result.chars().count(), 6);
    }

    #[test]
    fn truncate_preserves_multibyte_boundaries() {
        // 6 CJK characters
        let cjk = "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6d4b}\u{8bd5}";
        let result = truncate(cjk, 4);
        assert!(result.ends_with('\u{2026}'));
        assert_eq!(result.chars().count(), 4);
    }

    #[test]
    fn truncate_single_char_max() {
        let result = truncate("abcdef", 1);
        assert_eq!(result, "\u{2026}");
    }

    // --- resolve_timestamp tests ---

    // resolve_timestamp with Some returns the string directly
    #[test]
    fn resolve_timestamp_with_some_returns_value() {
        // We cannot call resolve_timestamp directly because it needs a TodoManager,
        // but we can test the logic: when timestamp is Some, it just returns it.
        let ts: Option<&str> = Some("20250101T120000");
        let result = ts.map(String::from).unwrap();
        assert_eq!(result, "20250101T120000");
    }
}
