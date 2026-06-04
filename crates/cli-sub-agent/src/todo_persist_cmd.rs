use anyhow::Result;
use csa_todo::{EpicPlan, GeneratedPlanPersistRequest, SpecDocument, TodoManager};

use crate::cli::TodoCommands;

pub(crate) fn handle_command(cmd: TodoCommands) -> Result<()> {
    let TodoCommands::Persist {
        timestamp,
        todo_file,
        spec_file,
        epic_plan_file,
        message,
        cd,
    } = cmd
    else {
        unreachable!("todo_persist_cmd only handles TodoCommands::Persist")
    };

    handle_persist(timestamp, todo_file, spec_file, epic_plan_file, message, cd)
}

pub(crate) fn handle_persist(
    timestamp: String,
    todo_file: String,
    spec_file: String,
    epic_plan_file: Option<String>,
    message: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    let todo_content = std::fs::read_to_string(&todo_file)
        .map_err(|e| anyhow::anyhow!("failed to read TODO file '{}': {}", todo_file, e))?;
    let spec_content = std::fs::read_to_string(&spec_file)
        .map_err(|e| anyhow::anyhow!("failed to read spec file '{}': {}", spec_file, e))?;
    let spec: SpecDocument = toml::from_str(&spec_content)
        .map_err(|e| anyhow::anyhow!("failed to parse spec file '{}': {}", spec_file, e))?;
    let epic_plan: Option<EpicPlan> = epic_plan_file
        .as_deref()
        .map(|path| {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read epic plan file '{}': {}", path, e))?;
            toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("failed to parse epic plan file '{}': {}", path, e))
        })
        .transpose()?;

    // Serialize the file writes, the git commit, and the hook-trigger decision
    // inside ONE hold of the TODO write lock: persist_generated_plan_with runs
    // the commit closure under the held lock, so a concurrent TODO writer cannot
    // overwrite the freshly written files between the write and the commit
    // (TOCTOU lost-update / wrong-snapshot hook). The closure computes the
    // commit message from the loaded plan, stages + commits, and returns the
    // commit hash; the TodoSave hook fires from that hash AFTER the lock is
    // released, so an arbitrary user hook command cannot deadlock on the lock.
    let todos_dir = manager.todos_dir();
    let (persisted, (commit_msg, commit_hash)) = manager.persist_generated_plan_with(
        &timestamp,
        GeneratedPlanPersistRequest {
            todo_content: &todo_content,
            spec: &spec,
            epic_plan: epic_plan.as_ref(),
        },
        |result| -> Result<(String, Option<String>)> {
            let commit_msg = message
                .clone()
                .unwrap_or_else(|| format!("persist: {}", result.plan.metadata.title));
            csa_todo::git::ensure_git_init(todos_dir)?;
            let file_refs: Vec<&str> = result.changed_files.iter().map(String::as_str).collect();
            let hash = csa_todo::git::save_files(todos_dir, &timestamp, &file_refs, &commit_msg)?;
            Ok((commit_msg, hash))
        },
    )?;

    match commit_hash {
        Some(hash) => {
            eprintln!("Persisted plan '{timestamp}' ({hash})");
            crate::todo_hooks::emit_todo_save_hook(
                &project_root,
                manager.todos_dir(),
                &timestamp,
                &commit_msg,
            );
        }
        None => eprintln!("Persisted plan '{timestamp}' (no git changes)"),
    }
    println!("{}", persisted.plan.todo_md_path().display());

    Ok(())
}

#[cfg(test)]
#[path = "todo_persist_cmd_tests.rs"]
mod tests;
