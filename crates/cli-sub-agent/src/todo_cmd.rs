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

    // Auto-commit the initial plan
    let commit_msg = format!("create: {}", title);
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, &commit_msg)?;

    println!("{}", plan.timestamp);
    eprintln!(
        "Created TODO plan: {} ({})",
        plan.metadata.title, plan.timestamp
    );
    eprintln!("  Path: {}", plan.todo_md_path().display());

    Ok(())
}

pub(crate) fn handle_save(
    timestamp: String,
    message: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let plan = manager.load(&timestamp)?;

    let commit_msg = message.unwrap_or_else(|| format!("update: {}", plan.metadata.title));
    let hash = csa_todo::git::save(manager.todos_dir(), &timestamp, &commit_msg)?;

    eprintln!("Saved {} ({})", timestamp, hash);

    Ok(())
}

pub(crate) fn handle_diff(
    timestamp: String,
    revision: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    // Validate the plan exists
    manager.load(&timestamp)?;

    let diff_output = csa_todo::git::diff(manager.todos_dir(), &timestamp, revision.as_deref())?;

    if diff_output.is_empty() {
        eprintln!("No changes.");
    } else {
        print!("{diff_output}");
    }

    Ok(())
}

pub(crate) fn handle_history(timestamp: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    // Validate the plan exists
    manager.load(&timestamp)?;

    let log_output = csa_todo::git::history(manager.todos_dir(), &timestamp)?;

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

pub(crate) fn handle_show(timestamp: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let plan = manager.load(&timestamp)?;

    let content = std::fs::read_to_string(plan.todo_md_path())?;
    print!("{content}");

    Ok(())
}

pub(crate) fn handle_status(timestamp: String, status: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let new_status: TodoStatus = status.parse()?;

    let plan = manager.update_status(&timestamp, new_status)?;

    eprintln!(
        "Updated {} status → {}",
        plan.timestamp, plan.metadata.status
    );

    Ok(())
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
