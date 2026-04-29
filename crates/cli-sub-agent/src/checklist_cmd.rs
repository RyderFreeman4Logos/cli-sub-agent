use std::fs;
use std::path::Path;
use std::process::Command;

use crate::cli::ChecklistCommands;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use csa_core::checklist::{CheckStatus, ChecklistDocument, ChecklistItem, ChecklistMeta};
use csa_session::ChecklistStore;

pub(crate) fn handle_checklist_command(command: ChecklistCommands) -> Result<()> {
    match command {
        ChecklistCommands::Show { cd, branch } => handle_checklist_show(cd, branch),
        ChecklistCommands::Check {
            id,
            evidence,
            reviewer,
            cd,
            branch,
        } => handle_checklist_check(id, evidence, reviewer, cd, branch),
        ChecklistCommands::Reset { id, cd, branch } => handle_checklist_reset(id, cd, branch),
        ChecklistCommands::Generate {
            profile,
            scope,
            cd,
            branch,
        } => handle_checklist_generate(profile, scope, cd, branch),
        ChecklistCommands::List { cd } => handle_checklist_list(cd),
    }
}

pub(crate) fn handle_checklist_show(cd: Option<String>, branch: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let branch = resolve_branch(&project_root, branch)?;
    let store = ChecklistStore::new()?;
    let Some(doc) = store.load(&project_root, &branch)? else {
        println!("No checklist for branch '{branch}'.");
        println!(
            "Run `csa checklist generate --branch {}` to create one.",
            shell_words_safe(&branch)
        );
        return Ok(());
    };

    print_document(&doc);
    Ok(())
}

pub(crate) fn handle_checklist_check(
    id: String,
    evidence: String,
    reviewer: Option<String>,
    cd: Option<String>,
    branch: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let branch = resolve_branch(&project_root, branch)?;
    let reviewer = reviewer
        .or_else(|| std::env::var("CSA_TOOL").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    ChecklistStore::new()?.check_item(&project_root, &branch, &id, &evidence, &reviewer)?;
    println!("Checked {id}");
    Ok(())
}

pub(crate) fn handle_checklist_reset(
    id: String,
    cd: Option<String>,
    branch: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let branch = resolve_branch(&project_root, branch)?;
    ChecklistStore::new()?.reset_item(&project_root, &branch, &id)?;
    println!("Reset {id}");
    Ok(())
}

pub(crate) fn handle_checklist_generate(
    profile: Option<String>,
    scope: Option<String>,
    cd: Option<String>,
    branch: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let branch = resolve_branch(&project_root, branch)?;
    let profile = profile.unwrap_or_else(|| "rust".to_string());
    let criteria = generate_criteria(&project_root, &profile)?;
    let doc = ChecklistDocument {
        meta: ChecklistMeta {
            project_root: project_root.display().to_string(),
            branch: branch.clone(),
            created_at: Utc::now().to_rfc3339(),
            scope: scope.unwrap_or_default(),
            profile,
        },
        criteria,
    };

    let store = ChecklistStore::new()?;
    store.save(&project_root, &branch, &doc)?;
    println!(
        "Generated {} checklist criteria at {}",
        doc.criteria.len(),
        store.checklist_path(&project_root, &branch).display()
    );
    Ok(())
}

pub(crate) fn handle_checklist_list(cd: Option<String>) -> Result<()> {
    let store = ChecklistStore::new()?;
    if let Some(cd) = cd {
        let project_root = crate::pipeline::determine_project_root(Some(&cd))?;
        let project_dir = store.project_dir(&project_root);
        print_project_checklists(&project_dir)?;
        return Ok(());
    }

    let project_dirs = store.list_projects()?;
    if project_dirs.is_empty() {
        println!("No active checklists.");
        return Ok(());
    }
    for project_dir in project_dirs {
        print_project_checklists(&project_dir)?;
    }
    Ok(())
}

fn print_project_checklists(project_dir: &Path) -> Result<()> {
    for checklist_path in find_checklist_paths(project_dir)? {
        print_checklist_summary(&checklist_path)?;
    }
    Ok(())
}

fn find_checklist_paths(project_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut stack = vec![project_dir.to_path_buf()];
    let mut checklists = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to read checklist project dir: {}", dir.display())
                });
            }
        };
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if entry.file_name() == "checklist.toml" {
                checklists.push(path);
            }
        }
    }
    checklists.sort();
    Ok(checklists)
}

fn print_checklist_summary(checklist_path: &Path) -> Result<()> {
    let content = fs::read_to_string(checklist_path)
        .with_context(|| format!("Failed to read checklist: {}", checklist_path.display()))?;
    let doc: ChecklistDocument = toml::from_str(&content)
        .with_context(|| format!("Failed to parse checklist: {}", checklist_path.display()))?;
    let summary = doc.summary();
    println!(
        "{} {} checked={} unchecked={} failed={} na={}",
        doc.meta.project_root,
        doc.meta.branch,
        summary.checked,
        summary.unchecked,
        summary.failed,
        summary.not_applicable
    );
    Ok(())
}

fn print_document(doc: &ChecklistDocument) {
    let summary = doc.summary();
    println!("project: {}", doc.meta.project_root);
    println!("branch: {}", doc.meta.branch);
    println!("profile: {}", doc.meta.profile);
    if !doc.meta.scope.is_empty() {
        println!("scope: {}", doc.meta.scope);
    }
    println!(
        "summary: checked={} unchecked={} failed={} na={} all_checked={}",
        summary.checked,
        summary.unchecked,
        summary.failed,
        summary.not_applicable,
        doc.all_checked()
    );
    for item in &doc.criteria {
        println!(
            "- [{}] {} ({}) {}",
            status_label(item.status),
            item.id,
            item.source,
            item.description
        );
        if !item.evidence.is_empty() {
            println!("  evidence: {}", item.evidence);
        }
        if !item.reviewer.is_empty() {
            println!("  reviewer: {}", item.reviewer);
        }
    }
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Unchecked => "unchecked",
        CheckStatus::Checked => "checked",
        CheckStatus::Failed => "failed",
        CheckStatus::NotApplicable => "na",
    }
}

fn resolve_branch(project_root: &Path, branch: Option<String>) -> Result<String> {
    if let Some(branch) = branch {
        if branch.trim().is_empty() {
            bail!("branch cannot be empty");
        }
        return Ok(branch);
    }
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(project_root)
        .output()
        .with_context(|| format!("Failed to detect git branch in {}", project_root.display()))?;
    if !output.status.success() {
        bail!("Failed to detect git branch in {}", project_root.display());
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        bail!("Current checkout is detached; pass --branch explicitly");
    }
    Ok(branch)
}

fn generate_criteria(project_root: &Path, profile: &str) -> Result<Vec<ChecklistItem>> {
    let agents_path = project_root.join("AGENTS.md");
    let content = fs::read_to_string(&agents_path)
        .with_context(|| format!("Failed to read {}", agents_path.display()))?;
    let sections = sections_for_profile(profile);
    let mut current_section = String::new();
    let mut criteria = Vec::new();

    for line in content.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            current_section = section.trim().to_string();
            continue;
        }
        if !sections.iter().any(|section| *section == current_section) {
            continue;
        }
        let Some(item) = parse_rule_line(&current_section, line) else {
            continue;
        };
        criteria.push(item);
    }

    if criteria.is_empty() {
        bail!(
            "No checklist criteria found in {} for profile '{}'",
            agents_path.display(),
            profile
        );
    }
    Ok(criteria)
}

fn sections_for_profile(profile: &str) -> Vec<&'static str> {
    let mut sections = vec!["Meta (Agent Behavior)", "Design", "Practice"];
    match profile.to_ascii_lowercase().as_str() {
        "rust" => sections.push("Rust"),
        "python" => sections.push("Python"),
        "go" => sections.push("Go"),
        "node" | "typescript" | "ts" => sections.push("TypeScript"),
        "mixed" | "all" => sections.extend(["Rust", "Go", "Python", "TypeScript", "gRPC / Proto"]),
        _ => {}
    }
    sections
}

fn parse_rule_line(section: &str, line: &str) -> Option<ChecklistItem> {
    let trimmed = line.trim();
    let (id, rest) = trimmed.split_once('|')?;
    let id = id.trim();
    if id.len() != 3 || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (name, summary) = rest.split_once('|')?;
    let section_slug = section
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let rule_name = name.trim();
    let item_id = format!("{section_slug}-{id}-{rule_name}");
    Some(ChecklistItem {
        id: item_id,
        source: format!("{section} {id}|{rule_name}"),
        description: summary.trim().to_string(),
        status: CheckStatus::Unchecked,
        evidence: String::new(),
        reviewer: String::new(),
        checked_at: String::new(),
    })
}

fn shell_words_safe(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
    {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_rule_line, sections_for_profile};

    #[test]
    fn parse_rule_line_extracts_checklist_item() {
        let item = parse_rule_line(
            "Rust",
            "002|error-handling|NEVER unwrap() in library, propagate with ?",
        )
        .expect("rule parsed");

        assert_eq!(item.id, "rust-002-error-handling");
        assert_eq!(item.source, "Rust 002|error-handling");
        assert_eq!(
            item.description,
            "NEVER unwrap() in library, propagate with ?"
        );
    }

    #[test]
    fn rust_profile_includes_shared_and_rust_sections() {
        let sections = sections_for_profile("rust");
        assert!(sections.contains(&"Meta (Agent Behavior)"));
        assert!(sections.contains(&"Design"));
        assert!(sections.contains(&"Practice"));
        assert!(sections.contains(&"Rust"));
        assert!(!sections.contains(&"Python"));
    }
}
