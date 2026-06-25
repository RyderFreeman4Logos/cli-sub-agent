use std::path::Path;

use anyhow::Result;
use csa_todo::{
    EpicPlan, GeneratedPlanPersistRequest, SpecDocument, TodoManager, parse_spec_document,
    validate_generated_plan_request,
};

use crate::cli::TodoCommands;

pub(crate) fn handle_command(cmd: TodoCommands) -> Result<()> {
    let TodoCommands::Persist {
        timestamp,
        todo_file,
        spec_file,
        epic_plan_file,
        message,
        dry_run,
        cd,
    } = cmd
    else {
        unreachable!("todo_persist_cmd only handles TodoCommands::Persist")
    };

    handle_persist(
        timestamp,
        todo_file,
        spec_file,
        epic_plan_file,
        message,
        dry_run,
        cd,
    )
}

pub(crate) fn handle_persist(
    timestamp: String,
    todo_file: String,
    spec_file: String,
    epic_plan_file: Option<String>,
    message: Option<String>,
    dry_run: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;

    let todo_content = std::fs::read_to_string(&todo_file)
        .map_err(|e| anyhow::anyhow!("failed to read TODO file '{}': {}", todo_file, e))?;
    let spec = load_spec_document(&spec_file)?;
    let epic_plan: Option<EpicPlan> = epic_plan_file
        .as_deref()
        .map(|path| {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read epic plan file '{}': {}", path, e))?;
            toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("failed to parse epic plan file '{}': {}", path, e))
        })
        .transpose()?;
    let request = GeneratedPlanPersistRequest {
        todo_content: &todo_content,
        spec: &spec,
        epic_plan: epic_plan.as_ref(),
    };
    if dry_run {
        validate_generated_plan_request(&timestamp, &request)?;
        println!("validated generated plan artifacts for {timestamp}");
        return Ok(());
    }

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
        request,
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

fn load_spec_document(spec_file: &str) -> Result<SpecDocument> {
    let spec_content = std::fs::read_to_string(spec_file)
        .map_err(|e| anyhow::anyhow!("failed to read spec file '{}': {}", spec_file, e))?;
    match parse_spec_document(&spec_content, spec_file) {
        Ok(spec) => Ok(spec),
        Err(parse_error) => recover_spec_document_from_raw_artifact(spec_file, parse_error),
    }
}

fn recover_spec_document_from_raw_artifact(
    spec_file: &str,
    parse_error: anyhow::Error,
) -> Result<SpecDocument> {
    let raw_spec_file = Path::new(spec_file).with_file_name("spec.raw.txt");
    if !raw_spec_file.exists() {
        return Err(parse_error);
    }

    let raw_source = raw_spec_file.display().to_string();
    let raw_content = std::fs::read_to_string(&raw_spec_file).map_err(|e| {
        anyhow::anyhow!(
            "{}; failed to read raw spec artifact '{}': {}",
            parse_error,
            raw_source,
            e
        )
    })?;
    let recovered = extract_unambiguous_raw_spec(&raw_content, &raw_source).map_err(|e| {
        anyhow::anyhow!(
            "{}; raw spec recovery failed for '{}': {}",
            parse_error,
            raw_source,
            e
        )
    })?;
    Ok(recovered)
}

fn extract_unambiguous_raw_spec(raw_content: &str, raw_source: &str) -> Result<SpecDocument> {
    let mut recovered_specs: Vec<SpecDocument> = Vec::new();
    for (label, candidate) in raw_spec_candidates(raw_content) {
        let candidate = trim_candidate(&candidate);
        if candidate.is_empty() {
            continue;
        }
        let candidate_source = format!("{raw_source} ({label})");
        if let Ok(spec) = parse_spec_document(candidate, &candidate_source)
            && !recovered_specs.iter().any(|existing| existing == &spec)
        {
            recovered_specs.push(spec);
        }
    }

    match recovered_specs.len() {
        1 => Ok(recovered_specs.remove(0)),
        0 => anyhow::bail!("no raw/fenced TOML spec candidate was found"),
        count => anyhow::bail!(
            "found {count} different TOML spec candidates; refusing ambiguous recovery"
        ),
    }
}

fn raw_spec_candidates(raw_content: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    candidates.extend(fenced_toml_candidates(raw_content));
    candidates.extend(csa_section_candidates(raw_content));
    candidates.push(("full raw artifact".to_string(), raw_content.to_string()));
    candidates
}

fn fenced_toml_candidates(raw_content: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut open_fence: Option<(bool, Vec<&str>)> = None;
    for line in raw_content.lines() {
        if let Some(tag) = markdown_fence_tag(line) {
            if let Some((accept, body)) = open_fence.take() {
                if accept {
                    candidates.push(("fenced TOML".to_string(), body.join("\n")));
                }
            } else {
                open_fence = Some((is_toml_fence_tag(tag), Vec::new()));
            }
            continue;
        }
        if let Some((_, body)) = open_fence.as_mut() {
            body.push(line);
        }
    }
    candidates
}

fn markdown_fence_tag(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("```").map(str::trim)
}

fn is_toml_fence_tag(tag: &str) -> bool {
    let language = tag.split_whitespace().next().unwrap_or_default();
    language.is_empty()
        || language.eq_ignore_ascii_case("toml")
        || language.eq_ignore_ascii_case("spec.toml")
}

fn csa_section_candidates(raw_content: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut section_body: Option<Vec<&str>> = None;
    for line in raw_content.lines() {
        if section_body.is_none() && is_csa_section_start(line) {
            section_body = Some(Vec::new());
            continue;
        }
        if let Some(body) = section_body.as_mut() {
            if is_csa_section_end(line) {
                candidates.push(("CSA section".to_string(), body.join("\n")));
                section_body = None;
            } else {
                body.push(line);
            }
        }
    }
    candidates
}

fn is_csa_section_start(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<!--") && trimmed.contains("CSA:SECTION:") && !trimmed.contains(":END")
}

fn is_csa_section_end(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<!--") && trimmed.contains("CSA:SECTION:") && trimmed.contains(":END")
}

fn trim_candidate(candidate: &str) -> &str {
    candidate.trim_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
}

#[cfg(test)]
#[path = "todo_persist_cmd_tests.rs"]
mod tests;
