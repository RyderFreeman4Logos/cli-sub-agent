use anyhow::{Context, Result};
use csa_todo::TodoManager;

const MAX_REF_FILE_SIZE: u64 = 5_242_880; // 5MB

pub(crate) fn handle_ref_list(
    timestamp: Option<String>,
    tokens: bool,
    json: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = crate::todo_cmd::resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let index = manager.list_references(&plan, tokens)?;

    if json {
        let json_files: Vec<_> = index
            .files
            .iter()
            .map(|f| {
                let mut entry = serde_json::json!({
                    "name": f.name,
                    "size_bytes": f.size_bytes,
                });
                if let Some(t) = f.token_estimate {
                    entry["token_estimate"] = serde_json::json!(t);
                }
                entry
            })
            .collect();
        let output = serde_json::json!({
            "plan": ts,
            "files": json_files,
            "total_tokens": index.total_tokens,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if index.files.is_empty() {
        eprintln!("No reference files for plan '{ts}'.");
        return Ok(());
    }

    if tokens {
        println!("{:<30}  {:>10}  {:>10}", "NAME", "SIZE", "TOKENS");
        for entry in &index.files {
            println!(
                "{:<30}  {:>10}  {:>10}",
                entry.name,
                format_size(entry.size_bytes),
                entry
                    .token_estimate
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        if let Some(total) = index.total_tokens {
            println!();
            println!("Total: ~{total} tokens");
        }
    } else {
        println!("{:<30}  {:>10}", "NAME", "SIZE");
        for entry in &index.files {
            println!("{:<30}  {:>10}", entry.name, format_size(entry.size_bytes));
        }
    }

    Ok(())
}

pub(crate) fn handle_ref_show(
    timestamp: Option<String>,
    name: String,
    max_tokens: usize,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = crate::todo_cmd::resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let content = manager.read_reference(&plan, &name, Some(max_tokens))?;
    print!("{content}");

    Ok(())
}

pub(crate) fn handle_ref_add(
    timestamp: Option<String>,
    name: String,
    content_arg: Option<String>,
    file_arg: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = crate::todo_cmd::resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let content = if let Some(text) = content_arg {
        text
    } else if let Some(path_str) = file_arg {
        let path = std::path::Path::new(&path_str);
        let metadata =
            std::fs::metadata(path).with_context(|| format!("Failed to stat file: {path_str}"))?;
        if metadata.len() > MAX_REF_FILE_SIZE {
            anyhow::bail!(
                "File too large: {} bytes (max: {} bytes / 5MB)",
                metadata.len(),
                MAX_REF_FILE_SIZE
            );
        }
        std::fs::read_to_string(path).with_context(|| format!("Failed to read file: {path_str}"))?
    } else {
        // Read from stdin
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("Failed to read from stdin")?;
        buf
    };

    manager.write_reference(
        &plan,
        &name,
        &content,
        Some(csa_todo::ReferenceSource::Manual),
    )?;

    let refs_dir = plan.references_dir();
    eprintln!("Added reference '{name}' to plan '{ts}'.");
    eprintln!("  Path: {}", refs_dir.join(&name).display());

    Ok(())
}

pub(crate) fn handle_ref_import_transcript(
    timestamp: Option<String>,
    tool: String,
    session: String,
    name_override: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = crate::todo_cmd::resolve_timestamp(&manager, timestamp.as_deref())?;
    let plan = manager.load(&ts)?;

    let raw_content = csa_todo::xurl_integration::import_transcript(&tool, &session)?;

    let result = csa_todo::redact::redact_all(&raw_content);

    let session_short = if session.len() > 8 {
        &session[..8]
    } else {
        &session
    };
    let ref_name = name_override.unwrap_or_else(|| format!("transcript-{tool}-{session_short}.md"));

    manager.write_reference(
        &plan,
        &ref_name,
        &result.content,
        Some(csa_todo::ReferenceSource::Transcript {
            tool: tool.clone(),
            session: session.clone(),
        }),
    )?;

    eprintln!("Imported transcript as '{ref_name}' for plan '{ts}'.");
    eprintln!(
        "Redacted {} known pattern(s), flagged {} high-entropy string(s).",
        result.patterns_redacted,
        result.high_entropy_flagged.len()
    );
    if !result.high_entropy_flagged.is_empty() {
        eprintln!("High-entropy strings (review manually):");
        for (line, s) in &result.high_entropy_flagged {
            let display = if s.len() > 40 {
                format!("{}...", &s[..40])
            } else {
                s.clone()
            };
            eprintln!("  line {line}: {display}");
        }
    }

    Ok(())
}

/// Format byte size as human-readable string.
pub(crate) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1_048_576 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    }
}
