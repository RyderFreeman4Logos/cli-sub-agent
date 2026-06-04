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

    // Serialize the file writes, the git commit, the saved-version count, and
    // the hook-trigger decision inside ONE hold of the TODO write lock:
    // persist_generated_plan_with runs the commit closure under the held lock,
    // so a concurrent TODO writer cannot overwrite the freshly written files
    // between the write and the commit (TOCTOU lost-update / wrong-snapshot
    // hook). The closure computes the commit message from the loaded plan,
    // stages + commits, counts this commit's saved versions, and returns the
    // commit hash + version; the TodoSave hook fires from those captured values
    // AFTER the lock is released, so an arbitrary user hook command cannot
    // deadlock on the lock, yet the version still reflects exactly this save
    // (not a count a concurrent writer bumped post-release).
    let todos_dir = manager.todos_dir();
    let (persisted, (commit_msg, commit_hash, version)) = manager.persist_generated_plan_with(
        &timestamp,
        GeneratedPlanPersistRequest {
            todo_content: &todo_content,
            spec: &spec,
            epic_plan: epic_plan.as_ref(),
        },
        |result| -> Result<(String, Option<String>, usize)> {
            let commit_msg = message
                .clone()
                .unwrap_or_else(|| format!("persist: {}", result.plan.metadata.title));
            csa_todo::git::ensure_git_init(todos_dir)?;
            let file_refs: Vec<&str> = result.changed_files.iter().map(String::as_str).collect();
            let hash = csa_todo::git::save_files(todos_dir, &timestamp, &file_refs, &commit_msg)?;
            // Compute the saved-version count for THIS save while the write lock
            // is STILL held (right after the commit above), so the TodoSave hook
            // reports exactly this commit's version. Recomputing it after the
            // lock releases (the old behavior) let a concurrent TODO writer
            // commit another version first, making the hook report the later
            // count — the #1822 round-6 concurrency finding. Best-effort default
            // of 1 mirrors the prior hook behavior: the commit already succeeded,
            // so an informational version count must not fail the persist.
            let version = csa_todo::git::list_versions(todos_dir, &timestamp)
                .map(|versions| versions.len())
                .unwrap_or(1);
            Ok((commit_msg, hash, version))
        },
    )?;

    match commit_hash {
        Some(hash) => {
            eprintln!("Persisted plan '{timestamp}' ({hash})");
            crate::todo_hooks::emit_todo_save_hook(
                &project_root,
                manager.todos_dir(),
                &timestamp,
                version,
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
