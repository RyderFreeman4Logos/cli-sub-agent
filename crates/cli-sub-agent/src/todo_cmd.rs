use anyhow::Result;
use csa_todo::{TodoManager, TodoStatus};

pub(crate) fn handle_create(
    title: String,
    branch: Option<String>,
    cd: Option<String>,
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

    println!("{}", plan.timestamp);
    eprintln!(
        "Created TODO plan: {} ({})",
        plan.metadata.title, plan.timestamp
    );
    eprintln!("  Path: {}", plan.todo_md_path().display());

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
        Some(hash) => eprintln!("Saved {} ({})", ts, hash),
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
        print!("{diff_output}");
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

pub(crate) fn handle_list(status: Option<String>, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    let plans = if let Some(status_str) = status {
        let status: TodoStatus = status_str.parse()?;
        manager.find_by_status(status)?
    } else {
        manager.list()?
    };

    if plans.is_empty() {
        eprintln!("No TODO plans found.");
        return Ok(());
    }

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

    Ok(())
}

pub(crate) fn handle_find(
    branch: Option<String>,
    status: Option<String>,
    cd: Option<String>,
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
        eprintln!("No matching TODO plans found.");
        return Ok(());
    }

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
    } else if let Some(v) = version {
        let content = csa_todo::git::show_version(manager.todos_dir(), &ts, v)?;
        print!("{content}");
    } else {
        let content = std::fs::read_to_string(plan.todo_md_path())?;
        print!("{content}");
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

/// Truncate a string to `max_len` characters (not bytes), appending "…" if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
}
