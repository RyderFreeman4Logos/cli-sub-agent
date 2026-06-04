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

    let persisted = manager.persist_generated_plan(
        &timestamp,
        GeneratedPlanPersistRequest {
            todo_content: &todo_content,
            spec: &spec,
            epic_plan: epic_plan.as_ref(),
        },
    )?;

    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let file_refs: Vec<&str> = persisted.changed_files.iter().map(String::as_str).collect();
    let commit_msg =
        message.unwrap_or_else(|| format!("persist: {}", persisted.plan.metadata.title));
    match csa_todo::git::save_files(manager.todos_dir(), &timestamp, &file_refs, &commit_msg)? {
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
